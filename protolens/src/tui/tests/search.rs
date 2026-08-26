// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0114 §4: the `/` and `?` search prompt — where a pattern
//! matches, which way `n` repeats, and how case is folded.
//!
//! Both panes' searches are here rather than one each with its own pane:
//! they share `run_search`'s casing rules, and the smartcase tests
//! only convince side by side.

use super::super::search::SweepCursor;
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
/// override pane's own `SearchScope::Override`, not the main pane's
/// `SearchScope::Main` — the main-pane cursor never moves.
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
    app.node_text_mut()[1] = Some(Box::from("pkg.Overridden { x: 1 }"));

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
    let lower = SearchPattern::new("beta").expect("compiles");
    assert!(lower.find_range("beta: 1").is_some());
    assert!(lower.find_range("Beta: 1").is_some());
    assert!(lower.find_range("BETA: 1").is_some());

    let mixed = SearchPattern::new("Beta").expect("compiles");
    assert!(mixed.find_range("Beta: 1").is_some());
    assert!(mixed.find_range("beta: 1").is_none());
    assert!(mixed.find_range("BETA: 1").is_none());

    let upper = SearchPattern::new("BETA").expect("compiles");
    assert!(upper.find_range("BETA: 1").is_some());
    assert!(upper.find_range("beta: 1").is_none());
    assert!(upper.find_range("Beta: 1").is_none());
}

/// Spec 0273 test-plan items 6 and 7 (S6). Every non-path pattern is an
/// RE2 regex, so alternation, classes and anchors mean what they say —
/// and `^`/`$` are the *row's* ends, which is what `multi_line(true)`
/// buys on a haystack that will one day be wider than a row.
#[test]
fn a_regex_pattern_matches_by_its_syntax() {
    let alt = SearchPattern::new("(id|name):").expect("compiles");
    assert!(alt.find_range("  id: 7").is_some());
    assert!(alt.find_range("  name: x").is_some());
    assert!(alt.find_range("  other: 3").is_none());

    let anchored = SearchPattern::new(r"^\s*id").expect("compiles");
    assert!(anchored.find_range("  id: 7").is_some());
    assert!(anchored.find_range("  xid: 7").is_none());

    // `find_range_from` resumes with `find_at`, not on a slice, so the
    // anchor still knows the row began before `from`.
    assert_eq!(anchored.find_range_from("  id: 7", 2), None);
}

/// Spec 0273 test-plan item 8 (S7). The literal tier's needle comes from
/// the parsed HIR, not from the raw pattern text, so an escaped
/// metacharacter is the character — which is a thing a plain substring
/// search could not say at all.
#[test]
fn an_escaped_metacharacter_is_a_literal() {
    let dotted = SearchPattern::new(r"a\.b").expect("compiles");
    let SearchPattern::Literal { needle, .. } = &dotted else {
        panic!("an escaped literal stays on the literal tier");
    };
    assert_eq!(needle, "a.b");
    assert!(dotted.find_range("  x: a.b").is_some());
    assert!(dotted.find_range("  x: axb").is_none());
}

/// Spec 0273 test-plan items 10 and 11 (S9). Smartcase reads the parsed
/// pattern's *literals*, so a class escape — which is spelled with an
/// uppercase letter and contains no literal at all — does not make the
/// pattern case-sensitive. `(?-i)` is the escape hatch that takes case
/// back, which is what makes smartcase a default rather than a
/// restriction; vim spells the same thing `\C`.
#[test]
fn smartcase_reads_literals_and_yields_to_an_inline_flag() {
    let class = SearchPattern::new(r"\bid").expect("compiles");
    assert!(
        class.find_range("  ID: 7").is_some(),
        "the uppercase D of \\b..\\b is not a literal"
    );

    let forced = SearchPattern::new("(?-i)id").expect("compiles");
    assert!(forced.find_range("  id: 7").is_some());
    assert!(forced.find_range("  ID: 7").is_none());
}

/// Spec 0274 test-plan item 16 (N5). `\A` and `\z` stay refused, and
/// the reason has got stronger rather than weaker: the haystack is now a
/// segment, so they would bind to a boundary the reader cannot see and
/// that moves as the bake progresses.
///
/// Spec 0273's other refusal is gone. `id\nvalue` is the pattern this
/// spec exists for, and it now compiles.
#[test]
fn a_haystack_anchor_is_still_refused() {
    for pattern in [r"\Aid", r"id\z"] {
        let why = SearchPattern::new(pattern).expect_err(pattern);
        assert!(why.contains("^ and $"), "{pattern}: {why}");
    }
    // The line anchors themselves stay available, and mean what they
    // say now that the document has rows in it.
    assert!(SearchPattern::new("^id$").is_ok());
    for pattern in [r"id\nvalue", r"\n+", r"a(\n|\r\n)b"] {
        assert!(SearchPattern::new(pattern).is_ok(), "{pattern}");
    }
}

/// Spec 0274 test-plan items 1 and 2 (S2). Which engine a pattern
/// compiles to is decided by whether *some* string it matches contains a
/// newline, not by whether every one does.
///
/// The two questions disagree on exactly the patterns this routing
/// exists for: `a(\n|\r\n)b` needs a newline and `a\s*b` does not, yet
/// both can cross a row, and a reader cannot be asked to hold that
/// distinction.
#[test]
fn a_pattern_that_can_match_a_newline_takes_the_cursor_engine() {
    for pattern in [
        r"\s*id",
        "[^a]",
        "(?s)a.b",
        r"a\n?b",
        "(?s).*",
        r"[\x00-\xff]",
    ] {
        let compiled = SearchPattern::new(pattern).expect(pattern);
        assert!(
            matches!(compiled, SearchPattern::Multi(_)),
            "{pattern} admits a newline"
        );
    }
    // Nothing here can produce a `\n`, so reading the document a row at
    // a time cannot change the answer and 0273's path is kept.
    //
    // `.*` belongs here: `dot_matches_new_line(false)` (0273 S6) makes
    // `.` mean `[^\n]`, so only an explicit `(?s)` above lets it cross.
    for pattern in ["id", "[0-9]+", "(id|name):", r"a\.b", "^id$", r"\bid", ".*"] {
        let compiled = SearchPattern::new(pattern).expect(pattern);
        assert!(
            !matches!(compiled, SearchPattern::Multi(_)),
            "{pattern} admits no newline"
        );
    }
}

/// Spec 0274 S2: `hir_admits_newline` is an *any*-node question, but it
/// still cannot be `fold_hir` — a repetition that cannot repeat does not
/// admit what its sub-expression would.
///
/// The pattern is degenerate; the reason to pin it is that `fold_hir`
/// would descend into the sub-expression without asking, and the
/// resulting misroute is invisible in every other test.
#[test]
fn a_repetition_that_cannot_repeat_admits_nothing() {
    let never = SearchPattern::new(r"a\n{0}b").expect("compiles");
    assert!(
        !matches!(never, SearchPattern::Multi(_)),
        "`\\n{{0}}` matches only the empty string"
    );
}

/// Spec 0274 test-plan item 9 (S2, G2). The cursor engine is an
/// *optimization boundary*, not a second semantics: on a haystack with
/// no newline in it, both engines report the same first match.
///
/// This is what makes routing on `admits` safe. If it ever failed, the
/// reader's answer would depend on which engine their pattern happened
/// to reach.
#[test]
fn the_two_engines_agree_on_a_row_shaped_haystack() {
    let rows = [
        "  id: 7",
        "    name: \"alpha\"",
        "",
        "message {",
        "  x: a.b",
        "\ttabbed: 1",
    ];
    // Each pair is (cursor-engine pattern, an equivalent that stays on
    // 0273's path) — equivalent on a haystack that holds no newline.
    for (multi, plain) in [
        (r"\s*id", "[ \t]*id"),
        (r"[^a]", "[^a\n]"),
        (r"(?s)a.b", "a.b"),
        (r"a\n?b", "ab"),
    ] {
        let multi = SearchPattern::new(multi).expect("compiles");
        let plain = SearchPattern::new(plain).expect("compiles");
        assert!(matches!(multi, SearchPattern::Multi(_)));
        assert!(!matches!(plain, SearchPattern::Multi(_)));
        for row in rows {
            assert_eq!(
                multi.find_range(row),
                plain.find_range(row),
                "{row:?} disagreed"
            );
        }
    }
}

/// Spec 0274 S3: the cursor engine resumes with `set_start`, which is
/// this engine's `find_at`. Narrowing where a search may *begin* is not
/// the same as slicing the haystack — a slice would move `^` to the
/// resume point and silently change what the pattern means.
#[test]
fn the_cursor_engine_resumes_without_moving_the_anchors() {
    let anchored = SearchPattern::new(r"^\s*id").expect("compiles");
    assert!(matches!(anchored, SearchPattern::Multi(_)));
    assert!(anchored.find_range("  id: 7").is_some());
    // Position 2 is not a line start, so `^` cannot bind there — which
    // is only true because the engine can still see the two bytes
    // before it.
    assert_eq!(anchored.find_range_from("  id: 7", 2), None);
}

/// Spec 0195 S2: the case fold runs per `char` and streams, because a
/// few characters lowercase to more than one — `İ` becomes `i` plus a
/// combining dot. A one-to-one fold would misalign the comparison from
/// that character onwards rather than merely missing this match.
#[test]
fn the_case_fold_handles_a_multi_character_lowercase_mapping() {
    let dotted = SearchPattern::new("i\u{307}d").expect("compiles");
    assert!(dotted.find_range("\u{130}d: 7").is_some());
}

/// Spec 0195 Background 4 / test plan 8: nothing trims the pattern, so a
/// leading space is part of it and does distinguish an indented token
/// from one that starts its own line. The 2026-07-27 report read as
/// though the space were dropped; it is not, and this pins that.
#[test]
fn a_leading_space_is_part_of_the_pattern() {
    let spaced = SearchPattern::new(" id").expect("compiles");
    assert!(spaced.find_range("  id: 7").is_some());
    assert!(spaced.find_range("id: 7").is_none());
}

/// Spec 0195 G2 in the main pane: a lowercase pattern reaches a
/// capitalized node, and a capitalized pattern walks straight past the
/// lowercase one.
#[test]
fn main_pane_search_is_smartcase() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "Beta: 3"]);

    app.run_search(SearchScope::Main, SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 1);
    // Folds onto the capitalized node rather than stopping at "beta".
    app.run_search(SearchScope::Main, SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 2);

    app.set_cursor(0);
    app.run_search(SearchScope::Main, SearchDir::Forward, "Beta");
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
    app.run_search(SearchScope::Override, SearchDir::Forward, "beta");
    assert_eq!(app.override_highlight, 1);

    app.override_highlight = 0;
    app.run_search(SearchScope::Override, SearchDir::Forward, "Beta");
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
/// Spec 0273 S5: a line has *one* haystack, chosen by the pattern's
/// shape. A path pattern stops once per node, on its first own line
/// (S4), and is compared segment-wise (S3) — spelled out here over the
/// rendered path string, so that the walk's `Vec<usize>` comparison is
/// held against something other than itself.
fn every_match_in_text_order(app: &App, pattern: &str) -> Vec<(usize, usize)> {
    let lines = app.document_lines();
    let needle = SearchPattern::new(pattern).expect("a test pattern compiles");
    let mut out = Vec::new();
    for (line, raw) in lines.iter().enumerate() {
        let Some(pos) = app.line_pos(line) else {
            continue;
        };
        if app.is_footer(pos) {
            continue;
        }
        // Apply the same display transforms as the search haystack so
        // that this helper and `run_search` agree on what each line says.
        let owner = (pos.line_in_node == 0).then_some(pos.node);
        let text = app.line_display_text(raw, owner);
        // Spec 0246 S3a: a stop sits at the column its caret lands on,
        // never left of the row's first non-blank.
        let indent = text.len() - text.trim_start().len();
        if needle.is_path() {
            if pos.line_in_node == 0 && path_has_prefix(&app.positional_path(pos.node), pattern) {
                out.push((line, indent));
            }
            continue;
        }
        let mut from = 0;
        while let Some(range) = needle.find_range_from(&text, from) {
            out.push((line, range.start.max(indent)));
            from = range.start + text[range.start..].chars().next().map_or(1, char::len_utf8);
        }
        out.dedup();
    }
    out
}

/// Whether `path` — a rendered `/1/2/3` — begins with `pattern`'s
/// segments. Spec 0273 S3, by splitting rather than by comparing the
/// walk's own segment vectors.
fn path_has_prefix(path: &str, pattern: &str) -> bool {
    let split = |s: &str| {
        s.trim_end_matches('/')
            .split('/')
            .skip(1)
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let wanted = split(pattern);
    let have = split(path);
    wanted.len() <= have.len() && wanted.iter().zip(&have).all(|(a, b)| a == b)
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
        // Spec 0273 S6 makes a bare `{` a repetition operator with
        // nothing to repeat, so the brace is escaped; S7 then folds it
        // back into the same literal search it always was.
        for pattern in [":", r"\{", "1"] {
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
                        app.run_search(SearchScope::Main, dir, pattern);
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

    app.run_search(SearchScope::Main, SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 2, "on the match's first character");
    assert_eq!(app.desired_column, 2);

    // Within the row the caret still follows the match.
    app.run_search(SearchScope::Main, SearchDir::Forward, "2");
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, "  beta: ".chars().count());

    // A pattern whose first character is in the indentation clamps.
    app.run_search(SearchScope::Main, SearchDir::Forward, " beta");
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

/// `Outer { repeated int32 vals = 1; }` carrying a packed run of `n`
/// elements, the k-th holding `k % 128` — one byte each, so the run's
/// length in the blob is `n`.
///
/// Spec 0216 S22 collapses the whole run into a *single* arena node, so
/// all `n` of those lines live in one node's text. That is the shape
/// `stepped_offset` exists for, and the only one in which a line's
/// position inside its owner is expensive to name.
fn packed_run_app(n: usize) -> App {
    use prost_types::field_descriptor_proto::{Label, Type};
    use prototext_core::helpers::WT_LEN;

    let fds = proto3_fds(
        "packed_run.proto",
        vec![message(
            "Outer",
            vec![field("vals", 1, Label::Repeated, Type::Int32)],
        )],
    );
    let mut blob = vec![((1u32 << 3) | WT_LEN) as u8];
    let mut len = n;
    while len >= 0x80 {
        blob.push((len as u8 & 0x7F) | 0x80);
        len >>= 7;
    }
    blob.push(len as u8);
    blob.extend((0..n).map(|k| (k % 128) as u8));

    let mut app = fixture_under("packed-run", &fds, "test.Outer", &blob);
    app.splash = false;
    app.term_width = 120;
    app.message.clear();
    app
}

/// Spec 0272 S1. The run's elements are lines of one node, and
/// `line_offset` names one by counting newlines from that node's start
/// — so asking it per candidate makes a walk across the run quadratic
/// in the run's length. `SearchSweep::offset` carries the answer
/// instead.
///
/// The bound is deliberately coarse. Carrying the cursor, this walk
/// measures 7.5 ms in release; re-deriving it, the same walk measured
/// 9.2 s, and a 128 000-element run — a size googleapis reaches — would
/// have taken minutes. Anything between those is a bug, and two seconds
/// is far enough from both to survive a debug build and a loaded
/// machine.
#[test]
fn a_sweep_across_a_packed_run_is_linear_in_its_length() {
    let mut app = packed_run_app(32_000);

    let started = std::time::Instant::now();
    type_keys(&mut app, "/zzzz");
    let slices = settle_sweep(&mut app);
    let elapsed = started.elapsed();

    assert_eq!(slices, 33, "32 002 candidates at 1 000 a slice");
    assert!(
        app.search_sweep.as_ref().unwrap().found.is_none(),
        "`zzzz` is in no line of a run of integers"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the walk must not rescan the run per line: {elapsed:?}"
    );
}

/// Spec 0272 S1, the other half: carrying the cursor has to arrive at
/// the same lines re-deriving it did.
///
/// Both directions, because they step it differently — forward scans to
/// the next newline, backward looks for the one before the current
/// line. A cursor that drifted by even a byte would slice the text at
/// the wrong place and the pattern would match nothing at all, so the
/// hit's own line index is the whole assertion.
#[test]
fn a_sweep_reaches_the_right_line_of_a_packed_run_both_ways() {
    for (dir, target, expected) in [('/', "vals: 77", 77u32), ('?', "vals: 12", 12)] {
        let mut app = packed_run_app(120);
        // Start in the middle, so a forward walk climbs through the run
        // and a backward one descends through it.
        app.cursor_line_in_node = 50;

        type_keys(&mut app, &format!("{dir}{target}"));
        settle_sweep(&mut app);

        let hit = app
            .search_sweep
            .as_ref()
            .and_then(|s| s.found)
            .unwrap_or_else(|| panic!("`{target}` is element {expected} of the run"));
        let SweepCursor::Line(pos) = hit.at else {
            panic!("the main pane's candidates are lines")
        };
        assert_eq!(
            pos.line_in_node, expected,
            "`{dir}{target}` must land on the run's own element {expected}"
        );
        // `contains` rather than equality: an element line is indented
        // and carries the run's `#@ repeated int32 [packed=true]`
        // annotation, and neither is what this is about.
        let text = app.line_text(pos);
        assert!(
            text.contains(target),
            "and that line must be the one the pattern matched: {text:?}"
        );
    }
}

/// A long document whose only occurrence of `target` is on `line`,
/// prefixed by `indent` spaces and followed by enough filler for the
/// horizontal centering to have somewhere to put it.
fn target_at(n: usize, line: usize, indent: usize) -> App {
    let mut app = wide_sibling_scalars_app(n);
    app.node_text_mut()[line] = Some(Box::from(
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
    let SearchPattern::Literal { needle, .. } = &sweep.pattern else {
        panic!("spec 0273 S7: a bare word is the literal tier");
    };
    assert_eq!(needle, "zzz");
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
    app.node_text_mut()[50] = Some(Box::from("fo: 0"));
    app.node_text_mut()[150] = Some(Box::from("foo: 0"));
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
/// yellow while the sweep is still running, default once it has a hit,
/// red only once it has finished with nothing. Spec 0235 tinted the
/// first and the third alike, which trained the user to ignore red on
/// any document big enough for the sweep to be visible.
#[test]
fn search_prompt_is_yellow_while_sweeping_and_red_when_finished() {
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

/// Spec 0272 S2. A miss taken while the bake still owes subtrees is not
/// the claim red makes — the sweep never saw those bodies. It draws in
/// the same gray the fold margin is using against those very
/// subtrees, and turns red only once there is nothing left unread.
///
/// The prompt and `App::not_found`'s message answer to one predicate,
/// so both halves of the report are asserted here together.
#[test]
fn an_unbaked_document_tints_a_miss_gray_rather_than_red() {
    let mut app = target_at(2500, 2000, 0);
    app.splash = false;
    app.term_width = 120;

    let pattern_style = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let area = app.cmd_area.expect("the prompt must be on screen");
        terminal.backend().buffer()[(area.x + 1, area.y)].style().fg
    };

    // A subtree the bake has not reached. `target_at`'s nodes are
    // leaves, so the set is what is asked about and not what is folded.
    app.auto_folded.insert(app.first_node);

    type_keys(&mut app, "/zzz");
    settle_sweep(&mut app);
    assert!(app.search_sweep.as_ref().unwrap().is_finished());
    assert_eq!(
        pattern_style(&mut app),
        theme::search_unbaked_style(app.theme).fg,
        "the sweep never read the folded bodies, so `not there` is not yet an answer"
    );
    assert!(
        app.not_found("zzz", SearchScope::Main)
            .contains("not yet baked"),
        "and the message says the same thing in words"
    );

    app.auto_folded.clear();
    assert_eq!(
        pattern_style(&mut app),
        theme::search_unmatched_style(app.theme).fg,
        "with the whole document read, the same miss is red"
    );
}

/// Spec 0273 test-plan item 1 (G1, S2, S5). A pattern of `/` and digits
/// searches paths and *only* paths. The path reaches a node no line's
/// text names, and a line whose text spells the same thing is not a stop
/// — which is the whole of what the shape test buys, and what spec 0235
/// S19's both-haystacks rule got wrong.
#[test]
fn a_pattern_of_digits_and_slashes_searches_paths_only() {
    let (mut app, _run, tail, a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");
    assert_eq!(app.positional_path(id), "/2/1");
    assert!(
        app.document_lines().iter().all(|l| !l.contains("/2/1")),
        "the path must be the only haystack that can reach it"
    );

    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);

    // Now a second line carries the same pattern in its *text*. It is
    // not a stop, so the next `n` wraps back onto the one node whose
    // path the pattern spells.
    app.node_text_mut()[a] = Some(Box::from("  a: \"/2/1\""));
    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id, "the text spelling the path is not a stop");
}

/// Spec 0273 test-plan item 5 (S5), the converse. A word pattern never
/// reaches the path haystack, so a node whose *path* would match a
/// regex is not a stop for it.
#[test]
fn a_word_pattern_never_stops_on_a_path() {
    let (mut app, _run, tail, _a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");
    assert_eq!(app.positional_path(id), "/2/1");

    app.set_cursor(app.first_node);
    // A regex that matches `/2/1` the string, typed without the shape
    // that would make it a path pattern.
    app.run_search(SearchScope::Main, SearchDir::Forward, "2.1");
    assert_ne!(app.cursor, id);
    assert!(app.message.contains("not found"));
}

/// Spec 0273 test-plan item 2 (S3), at the comparison itself. A path
/// prefix is compared segment by segment, so `/1` reaches `/1/23` and
/// neither `/12` nor `/2/1`. The old rule was a raw `str::starts_with`,
/// under which `/1` matched `/12`.
///
/// Stated against the segment vectors because their *direction* is the
/// trap: the pattern's run root-first and `PathScratch`'s leaf-first.
#[test]
fn a_path_prefix_is_compared_by_segment() {
    let one = SearchPattern::new("/1").expect("compiles");
    assert!(one.is_path());
    assert!(one.matches_path(&[1]), "/1");
    assert!(one.matches_path(&[23, 1]), "/1/23");
    assert!(!one.matches_path(&[12]), "/12 is not an extension of /1");
    assert!(!one.matches_path(&[1, 2]), "/2/1 is not one either");
    assert!(!one.matches_path(&[]), "the root is shorter than /1");

    // Spec 0273 S2: the shape test, whose whole job is telling these
    // apart by eye.
    for text in ["/", "/1", "/1/2", "/1/2/"] {
        assert!(
            SearchPattern::new(text).expect("compiles").is_path(),
            "{text} is a path pattern"
        );
    }
    for text in ["/1/a", "1/2", "//2", "/1 ", "/1|2"] {
        assert!(
            !SearchPattern::new(text).expect("compiles").is_path(),
            "{text} is not a path pattern"
        );
    }
}

/// Spec 0273 test-plan item 4 (S4). A node's own lines all carry the
/// same path, so a path pattern stops on it once — on its first line —
/// rather than turning a three-line node into three stops on one answer.
#[test]
fn a_path_stops_once_per_node() {
    let (mut app, run, ..) = packed_run_with_tail_fixture();
    assert_eq!(app.positional_path(run), "/1");
    assert!(
        app.tree[run].lines_visible > 1,
        "the packed run must own several lines for this to say anything"
    );

    app.set_cursor(app.first_node);
    for _ in 0..2 {
        app.run_search(SearchScope::Main, SearchDir::Forward, "/1");
        assert_eq!(app.cursor, run);
        assert_eq!(
            app.cursor_line_in_node, 0,
            "the run's later elements are not stops of their own"
        );
    }
}

/// Spec 0273 test-plan item 3 (S3). A bare `/` has no segments, so it is
/// a prefix of every path and walks every node. Useless and consistent;
/// consistency wins, and it costs the shape test no special case.
#[test]
fn a_bare_slash_walks_every_node() {
    let (mut app, run, tail, a, b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");

    app.set_cursor(app.first_node);
    let mut seen = std::collections::HashSet::from([app.cursor]);
    for _ in 0..8 {
        app.run_search(SearchScope::Main, SearchDir::Forward, "/");
        seen.insert(app.cursor);
    }
    for node in [run, tail, a, b, id] {
        assert!(seen.contains(&node), "`/` must stop on every node");
    }
}

/// Spec 0235 S19 as amended. The path haystack is matched *anchored*:
/// `2/1` is a suffix of `/2/1`, and that no longer counts. Only a
/// pattern that spells the path from the root reaches the node, so an
/// ordinary word search never touches the second haystack at all.
#[test]
fn a_path_matches_only_from_its_start() {
    let (mut app, _run, tail, _a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");
    assert_eq!(app.positional_path(id), "/2/1");

    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "2/1");
    assert_ne!(app.cursor, id, "a suffix of the path is not a match");
    assert!(app.message.contains("not found"));

    // A prefix of the path is a match — it reaches the ancestor it
    // spells first, then the node itself.
    app.message.clear();
    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2");
    assert_eq!(app.cursor, tail, "`/2` is the parent's whole path");
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2");
    assert_eq!(app.cursor, id);
    assert!(app.message.is_empty());

    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);
}

/// Spec 0235 test-plan item 20 (S20). A path is not on screen, so a path
/// match has no column of its own: it lands on the row's Home anchor. A
/// text match keeps the matched character and `Free`.
///
/// Spec 0273 S5 amended the third case: a line whose text spells its own
/// path used to take the text column, because both haystacks were tried.
/// Now the pattern's shape decides, and a path pattern lands on Home
/// whatever the row happens to say.
#[test]
fn a_path_match_lands_on_the_home_anchor() {
    let (mut app, _run, tail, a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");

    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(app.cursor_column, app.caret_bounds().0);

    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "vals");
    assert_eq!(app.caret_anchor, CaretAnchor::Free);
    assert_eq!(app.cursor_column, 2);

    // `a`'s path is `/3` and its text now holds `/3` too.
    app.node_text_mut()[a] = Some(Box::from("  a: \"/3\""));
    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, "/3");
    assert_eq!(app.cursor, a);
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(app.cursor_column, app.caret_bounds().0);
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

    app.run_search(SearchScope::Override, SearchDir::Forward, "/1");
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

/// Spec 0273 test-plan item 9 (S8). A pattern that does not compile is
/// not an error and not a search — `foo(` is what `foo(bar)` looks like
/// halfway through typing it, and spec 0272 rebuilds the pattern on
/// every keystroke. The diagnostic arrives at `Enter` and not before.
#[test]
fn an_uncompilable_pattern_leaves_the_view_alone() {
    let mut app = sibling_leaves_app(&["foo(bar): 1", "zz: 2"]);
    app.splash = false;
    app.term_width = 120;
    let before = app.cursor;

    type_keys(&mut app, "/foo(");
    assert!(app.search_sweep.is_none(), "no sweep to run");
    assert!(app.search_highlight_pattern().is_none(), "nothing tinted");
    assert_eq!(app.cursor, before, "and the view has not moved");
    assert!(app.message.is_empty(), "silence while the reader is typing");

    press(&mut app, KeyCode::Enter);
    assert!(
        app.message.starts_with("bad pattern:"),
        "the diagnostic arrives at Enter: {}",
        app.message
    );

    // Finishing the pattern searches for what it now says.
    app.message.clear();
    commit_search_by_key(&mut app, r"foo\(bar");
    assert_eq!(app.cursor, 0);
    assert!(app.message.is_empty());
}

/// Spec 0273 test-plan item 12 (S10). `search_highlight_pattern` is
/// called from `render` on every frame, and after spec 0273 building a
/// `SearchPattern` is a regex compile. Two frames over an unchanged
/// pattern must compile once — pointer equality is the only honest way
/// to say so.
#[test]
fn the_compiled_pattern_survives_a_frame() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;
    commit_search_by_key(&mut app, "(al|be)pha");

    let first = app.search_highlight_pattern().expect("a live highlight");
    let second = app.search_highlight_pattern().expect("a live highlight");
    assert!(
        std::rc::Rc::ptr_eq(&first, &second),
        "the second frame reused the first frame's compile"
    );

    // A different pattern is a different compile, or the cache would be
    // wrong rather than merely absent.
    app.set_last_search_for(SearchScope::Main, (SearchDir::Forward, "beta".to_string()));
    let third = app.search_highlight_pattern().expect("a live highlight");
    assert!(!std::rc::Rc::ptr_eq(&first, &third));
}

/// Spec 0246 test-plan item 1 (G5, S2, S3). `n` over a row carrying
/// three occurrences stops at each of them before the row changes, and
/// the cycle closes on the one it started from.
#[test]
fn n_stops_at_every_match_on_a_line() {
    let mut app = sibling_leaves_app(&["ab ab ab", "zz"]);

    let got: Vec<(usize, usize)> = (0..4)
        .map(|_| {
            app.run_search(SearchScope::Main, SearchDir::Forward, "ab");
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

    app.run_search(SearchScope::Main, SearchDir::Backward, "ab");
    assert_eq!((app.cursor, app.cursor_column), (1, 6));
}

/// Spec 0246 test-plan item 3 (S2, S3). The origin row is visited twice
/// and the two halves partition it, so a document with one match sends
/// `n` back to that match rather than reporting "not found".
#[test]
fn a_search_wraps_back_to_the_match_it_started_on() {
    let mut app = sibling_leaves_app(&["ab", "zz"]);
    app.message.clear();

    app.run_search(SearchScope::Main, SearchDir::Forward, "ab");
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

    app.run_search(SearchScope::Main, SearchDir::Forward, "aa");
    assert_eq!((app.cursor, app.cursor_column), (0, 1));
    app.run_search(SearchScope::Main, SearchDir::Forward, "aa");
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
            app.run_search(SearchScope::Main, SearchDir::Forward, "/");
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
            app.run_search(SearchScope::Manage, SearchDir::Forward, "z");
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

/// readline's `previous-history`/`next-history` reach the same history
/// as the arrows do, at a `/` prompt and at an `F`/`B` find prompt
/// alike — and, like the arrows, are unbound at a `:` prompt, where
/// there is no history to browse.
#[test]
fn ctrl_p_and_ctrl_n_alias_the_history_arrows() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");
    commit_search_by_key(&mut app, "beta");

    let ctrl =
        |app: &mut App, c| app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));

    type_keys(&mut app, "/");
    ctrl(&mut app, 'p');
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    ctrl(&mut app, 'p');
    assert_eq!(app.command_buffer.as_deref(), Some("alpha"));
    ctrl(&mut app, 'n');
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    press(&mut app, KeyCode::Esc);

    // The find prompt `F` opens is the same prompt and browses the same
    // history.
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE));
    ctrl(&mut app, 'p');
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    press(&mut app, KeyCode::Esc);

    // At a `:` prompt they are swallowed rather than typed: the buffer
    // gains neither a history entry nor a literal `p`.
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    ctrl(&mut app, 'p');
    assert_eq!(app.command_buffer.as_deref(), Some(""));
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
        CommandLineKind::search(SearchDir::Forward)
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
    app.node_text_mut()[100] = Some(Box::from("target: 0"));
    app.node_text_mut()[300] = Some(Box::from("target: 0"));
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
    let ascii = SearchPattern::new("beta").expect("compiles");
    assert!(ascii.find_range("BETA: 1").is_some());
    assert_eq!(ascii.find_range("x beta").map(|r| r.start), Some(2));

    // A non-ASCII needle skips it on the first guard.
    let accented = SearchPattern::new("é").expect("compiles");
    assert!(accented.find_range("café: 1").is_some());
    assert!(accented.find_range("CAFÉ: 1").is_some());

    // An ASCII needle against a non-ASCII haystack skips it on the
    // second, which is the guard a first-character test alone misses.
    let k = SearchPattern::new("k").expect("compiles");
    assert!(k.find_range("\u{212A}elvin: 1").is_some());
}

// ---------------------------------------------------------------------
// Spec 0276: the find prompt — `F`/`B`, `Enter` steps, `Esc` accepts.
// ---------------------------------------------------------------------

/// Open a find prompt and type `pattern` into it, one keystroke at a
/// time, then run the sweep out.
fn find_by_key(app: &mut App, key: char, pattern: &str) {
    press(app, KeyCode::Char(key));
    type_keys(app, pattern);
    settle_sweep(app);
}

/// Spec 0276 test-plan item 1 (G1, S2). `F` opens pre-filled with the
/// pane's last pattern and with a sweep already running — the one thing
/// `/` cannot do, having nothing yet to look for.
#[test]
fn f_opens_a_find_prompt_prefilled_with_the_last_pattern() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "beta");
    assert_eq!(app.cursor, 1);

    press(&mut app, KeyCode::Char('F'));
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    assert_eq!(
        app.command_kind,
        CommandLineKind::Search {
            dir: SearchDir::Forward,
            find: Some(SearchDir::Forward),
        }
    );
    settle_sweep(&mut app);
    // The next occurrence after the caret, which is `beta2`'s row.
    assert_eq!(app.search_current_cell(), Some((3, 0, 4, false)));
}

/// Spec 0276 test-plan item 2 (G2, S4). `Enter` steps the current match
/// and leaves everything else alone — the buffer, the caret in it and
/// the prompt itself all stay.
#[test]
fn enter_in_a_find_prompt_steps_to_the_next_match() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("beta"),
        "the prompt stays open"
    );

    // And round, since the rotation cycles rather than reporting a miss.
    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));

    // `B` steps the other way from where the caret still is.
    press(&mut app, KeyCode::Esc);
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 3;

    find_by_key(&mut app, 'B', "beta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));
    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));
}

/// Spec 0276 test-plan item 3 (G3, S5 as amended 2026-08-11). `Esc`
/// accepts, and the caret lands on the match's **first** character —
/// the same landing a `/` commit makes, which is what makes the two
/// gestures interchangeable.
#[test]
fn esc_accepts_a_find_at_the_start_of_the_match() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    press(&mut app, KeyCode::Esc);

    assert!(app.command_buffer.is_none(), "the prompt closes");
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 0);
    assert_eq!(app.caret_anchor, CaretAnchor::Free);
}

/// Spec 0276 test-plan item 4 (S5's second bullet, and N3). A hit that
/// crosses a row accepts on the row it *starts* on, like any other
/// search landing — and leaves no selection behind, unlike spec 0274
/// S12's `/` commit: the find's highlight already shows the extent.
#[test]
fn esc_accepts_a_cross_row_find_on_its_first_row() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', r"1\nbeta");
    press(&mut app, KeyCode::Esc);

    // The match runs from `alpha: 1`'s column 7 to `beta: 2`'s column 4.
    assert_eq!(app.cursor, 0);
    assert_eq!(app.cursor_column, 7);
    assert_eq!(app.select_anchor, None);
    assert!(!app.select_engaged);
}

/// Spec 0276 test-plan item 5 (G4, S6). An accepted find is a search
/// like any other: `n` repeats it, and `Up` at a later prompt recalls
/// it.
#[test]
fn an_accepted_find_is_repeatable_with_n() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.cursor, 1);
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        Some(&(SearchDir::Forward, "beta".to_string()))
    );

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 3, "{}", app.message);

    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
}

/// Spec 0276 test-plan item 6 (S7). A find showing no match has nothing
/// to accept, so its `Esc` is `/`'s unchanged: the view the prompt was
/// opened from comes back and the position is left alone.
#[test]
fn esc_on_a_find_with_no_match_restores_the_view() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 1;
    app.cursor_column = 2;

    find_by_key(&mut app, 'F', "nope");
    assert_eq!(app.search_current_cell(), None);

    press(&mut app, KeyCode::Esc);
    assert!(app.command_buffer.is_none());
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 2);
    assert!(app.search_sweep.is_none());
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        None,
        "nothing was accepted"
    );
}

/// Spec 0276 test-plan item 7 (S8, S9, G1). The same gesture in a side
/// pane, where a match is a whole entry: the highlight previews the
/// current match and `Enter` steps it — a side pane tints nothing, so
/// this is the only way the step shows — and `Esc` accepts by leaving
/// the highlight where it is.
#[test]
fn f_finds_in_the_manage_pane() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;

    for (item, ty) in items.iter().zip(["pkg.zz1", "pkg.zz2", "pkg.zz3"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    // Entry 0 is the auto-derived root, which carries no `z`; the three
    // activated ones are 1, 2 and 3.
    app.manage_highlight = 0;

    find_by_key(&mut app, 'F', "zz");
    assert_eq!(app.manage_highlight, 1, "{}", app.message);

    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.manage_highlight, 2);

    press(&mut app, KeyCode::Esc);
    assert!(app.command_buffer.is_none());
    assert_eq!(app.manage_highlight, 2);
    assert_eq!(
        app.last_search_for(SearchScope::Manage),
        Some(&(SearchDir::Forward, "zz".to_string()))
    );
}

// ---------------------------------------------------------------------
// Spec 0281: a find steps whichever way you point it.
// ---------------------------------------------------------------------

/// Press `code` with `Shift` — spec 0281 S3's relative pair.
fn press_shift(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT));
}

/// The prompt's prefix character, which spec 0281 S2 makes a readout of
/// the active direction.
fn prefix(app: &App) -> char {
    app.search_row_text()
        .expect("a prompt is open")
        .chars()
        .next()
        .expect("the prefix is the first character")
}

/// Spec 0281 test-plan item 1 (G2, S3). The Shift pair is relative to
/// the key that opened the prompt: `Shift-→` is *onward*, which after
/// `B` means backward through the document.
#[test]
fn shift_arrows_step_relative_to_the_find_that_opened_the_prompt() {
    // Matches on nodes 0, 2 and 3.
    let texts = ["beta: 1", "x: 2", "beta: 3", "beta: 4"];

    let mut app = sibling_leaves_app(&texts);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 3;
    find_by_key(&mut app, 'B', "beta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    press_shift(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(
        app.search_current_cell().map(|c| c.0),
        Some(0),
        "onward from a `B` prompt is backward through the document"
    );
    press_shift(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2), "and back");

    // The mirror: from an `F` prompt the same two keys point the other
    // way round.
    let mut app = sibling_leaves_app(&texts);
    app.splash = false;
    app.term_width = 120;
    find_by_key(&mut app, 'F', "beta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    press_shift(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
    press_shift(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));
}

/// Spec 0281 test-plan item 2 (G3, G4, S2, S5). A step aims the prompt:
/// the prefix says where the next `Enter` goes, and `Enter` goes there.
///
/// This is the whole of what `Ctrl-←` could not do before — it moved
/// the match and left the prompt pointing the other way.
#[test]
fn a_step_points_the_prompt_at_where_it_went() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 3;

    find_by_key(&mut app, 'B', "beta");
    assert_eq!(prefix(&app), '<');
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    // Back, from a backward find: forward through the document.
    press_shift(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(prefix(&app), '>');
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(
        app.search_current_cell().map(|c| c.0),
        Some(0),
        "`Enter` continues forward, wrapping, rather than doubling back"
    );

    // And onward again re-aims it at the default.
    press_shift(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(prefix(&app), '<');
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));
}

/// Spec 0281 test-plan item 3 (S4, N2). The Ctrl pair keeps its absolute
/// directions and gains only the aiming.
#[test]
fn ctrl_arrows_stay_absolute_and_set_the_active_direction() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    assert_eq!(prefix(&app), '>');
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    press_ctrl(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));
    assert_eq!(prefix(&app), '<', "the prompt now points where it went");

    press(&mut app, KeyCode::Enter);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
}

/// Spec 0281 test-plan item 4 (N1, S3). A `/` prompt is untouched: its
/// `Ctrl-←` still rotates without re-pointing anything, and `Shift-→`
/// is still the text caret.
#[test]
fn a_commit_prompt_has_no_active_direction() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/");
    type_keys(&mut app, "beta");
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));

    press_ctrl(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));
    assert_eq!(prefix(&app), '/', "still the forward search it opened as");

    press(&mut app, KeyCode::Home);
    press_shift(&mut app, KeyCode::Right);
    assert_eq!(
        app.command_cursor, 1,
        "`Shift-→` falls through to the text caret"
    );

    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        Some(&(SearchDir::Forward, "beta".to_string()))
    );
}

/// Spec 0281 test-plan item 5 (S2). An accepted find is committed in the
/// direction it last stepped, so `n` carries on that way.
#[test]
fn an_accepted_find_repeats_in_the_direction_it_last_stepped() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    press_shift(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.cursor, 0);
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        Some(&(SearchDir::Backward, "beta".to_string())),
        "the echo spells the last step, not the key that opened the prompt"
    );

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 3, "{}", app.message);
}

// ---------------------------------------------------------------------
// Spec 0277: a search says which match you are on.
// ---------------------------------------------------------------------

/// Run the tally's walk to the end, the way `run_loop`'s idle arm does.
fn settle_tally(app: &mut App) {
    for _ in 0..10_000 {
        if app.search_tally_step() == SweepStep::Idle {
            return;
        }
    }
    panic!("a tally must converge");
}

/// Commit `/pattern`, then settle both the sweep and the tally.
fn counted_search(app: &mut App, pattern: &str) {
    commit_search_by_key(app, pattern);
    settle_tally(app);
}

/// Spec 0277 test-plan item 1 (S4). The tally counts every match in the
/// document, which is neither what the sweep visits nor what one row
/// holds: `betabeta` is two stops on one row, and the match on the
/// origin's own row is one the sweep skipped on its way out.
#[test]
fn a_tally_counts_every_match_in_the_document() {
    let mut app = sibling_leaves_app(&["beta: 1", "x: 2", "betabeta: 3"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    // The sweep landed on `betabeta`'s first match, having skipped the
    // one under the caret — which the tally still counts, as the first
    // of the three.
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2));
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 3"));
}

/// Spec 0277 test-plan item 2 (G1, S3, S5). The ordinal counts from the
/// start of the *document*, not from where the search began — so a
/// search started in the middle reports the place its match holds in the
/// document rather than the place it holds in the sweep.
#[test]
fn the_ordinal_counts_from_the_start_of_the_document() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 2;

    counted_search(&mut app, "beta");
    // Sweep order made this the *first* match found; document order
    // makes it the last.
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));
    assert_eq!(app.search_tally_text().as_deref(), Some("4 of 4"));
}

/// Spec 0277 test-plan item 3 (S6). `n` steps the ordinal on the way
/// out, so the new number is right *before* the tally has taken a step —
/// a walk would have shown `? of 4` here.
#[test]
fn n_steps_the_ordinal_without_walking_again() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.search_tally_text().as_deref(), Some("3 of 4"));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.search_tally_text().as_deref(), Some("4 of 4"));
}

/// Spec 0277 test-plan item 4 (G3). The pair means what the reader
/// assumes: one more `n` past the last match is `1 of 4` again.
#[test]
fn n_wraps_the_ordinal_at_the_last_match() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 2;

    counted_search(&mut app, "beta");
    assert_eq!(app.search_tally_text().as_deref(), Some("4 of 4"));

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 0);
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 4"));
}

/// Spec 0277 test-plan item 5 (S3, S6). The first number counts forward
/// from the document's start whichever way the search runs, so a
/// backward step *decrements* it — and wraps the other way.
#[test]
fn a_backward_search_decrements_the_ordinal() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;
    app.cursor = 2;

    type_keys(&mut app, "?");
    type_keys(&mut app, "beta");
    press(&mut app, KeyCode::Enter);
    settle_tally(&mut app);
    assert_eq!(app.cursor, 1);
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 0);
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 4"));

    // And round: below the first match is the last one.
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 3);
    assert_eq!(app.search_tally_text().as_deref(), Some("4 of 4"));
}

/// Spec 0277 test-plan item 6 (S6's second bullet). An accepted find is
/// a departure like a `/` commit's — since spec 0276 S5's amendment the
/// two land identically — and the ordinal steps rather than being
/// re-derived.
#[test]
fn n_after_an_accepted_find_still_steps() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    press(&mut app, KeyCode::Esc);
    settle_tally(&mut app);
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 0);
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.search_tally_text().as_deref(), Some("3 of 4"));
}

/// Spec 0277 test-plan item 7 (S6, S7). A departure that carries no
/// ordinal — the reader walked off the match first — leaves the ordinal
/// unknown, and S7's prefix walk recovers it. The total survives
/// untouched, which is what the `?` says.
#[test]
fn moving_the_caret_off_the_match_then_pressing_n_re_derives() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));

    // Off the match, and off its row.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.cursor, 3);
    assert_eq!(
        app.search_tally_text().as_deref(),
        Some("? of 4"),
        "the total survives the ordinal"
    );

    settle_tally(&mut app);
    assert_eq!(app.search_tally_text().as_deref(), Some("4 of 4"));
}

/// Spec 0277 test-plan item 8 (G2, S8). Nothing is drawn until the total
/// exists — no keystroke and no sweep waits on the counting, and a
/// half-counted total is not shown at all.
#[test]
fn no_indication_while_the_total_is_still_walking() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3", "beta: 4"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "beta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(1));
    assert_eq!(
        app.search_tally_text(),
        None,
        "the answer arrived without the count"
    );

    // The step that starts the walk has still counted nothing.
    assert_eq!(app.search_tally_step(), SweepStep::Progressed);
    assert_eq!(app.search_tally_text(), None);

    settle_tally(&mut app);
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));
}

/// Spec 0277 test-plan item 9 (S11). The tally is keyed on the pattern,
/// so a keystroke drops both facts and starts a new walk rather than
/// leaving the previous pattern's total on the row.
#[test]
fn a_keystroke_restarts_the_tally() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta2: 2", "beta: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/");
    type_keys(&mut app, "beta");
    settle_sweep(&mut app);
    settle_tally(&mut app);
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 4"));

    type_keys(&mut app, "2");
    settle_sweep(&mut app);
    assert_eq!(app.search_tally_text(), None, "the old total is not kept");

    settle_tally(&mut app);
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 2"));
}

/// Spec 0277 test-plan item 10 (N1). A cross-row pattern's unit of work
/// is a segment scanned on a worker thread, and a tally queueing
/// segments would compete for that worker with the sweep the reader is
/// waiting on. An absent count, not a wrong one.
#[test]
fn a_cross_row_pattern_reports_no_count() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, r"1\nbeta");
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(0));
    assert_eq!(app.search_tally_text(), None);
}

/// Spec 0277 test-plan item 11 (N3). There is no `0 of 0`: a miss is
/// already reported on this same row by the pattern's own color and by
/// the message. One home per fact.
#[test]
fn a_miss_reports_no_count() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "nope");
    assert!(app.message.contains("not found"));
    assert_eq!(app.search_tally_text(), None);
}

/// Spec 0277 test-plan item 12 (S4). A side pane counts too, and there a
/// stop is a whole entry — spec 0246 N4 — however many times the pattern
/// occurs inside it.
#[test]
fn the_tally_counts_in_the_manage_pane() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;
    app.term_width = 120;

    for (item, ty) in items.iter().zip(["pkg.zz1", "pkg.zzzz2", "pkg.zz3"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    // Entry 0 is the auto-derived root, which carries no `z`.
    app.manage_highlight = 0;

    counted_search(&mut app, "zz");
    // Three entries match; the second holds two occurrences and is still
    // one stop.
    assert_eq!(app.manage_highlight, 1);
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 3"));

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.manage_highlight, 2);
    assert_eq!(app.search_tally_text().as_deref(), Some("2 of 3"));
}

// ---------------------------------------------------------------------
// Spec 0278: a committed search leaves its pattern on the row, and the
// count lives beside it.
// ---------------------------------------------------------------------

/// Spec 0278 test-plan item 1 (G1, S1). `Enter` closes the prompt but
/// leaves the pattern on the row, spelled the way the prompt had it.
#[test]
fn a_committed_search_echoes_its_pattern() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");

    assert!(app.command_buffer.is_none(), "the prompt closed");
    assert_eq!(app.search_row_text().as_deref(), Some("/beta"));
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 1"));

    // And on the row itself, not merely in the predicate: the pattern at
    // the left, the count right-aligned beside it.
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let area = app.cmd_area.expect("the command row must be on screen");
    let buffer = terminal.backend().buffer();
    // The whole row, not `cmd_area`: spec 0277 S8 gives the count a field
    // of its own, which `cmd_area` deliberately excludes.
    let row: String = (0..120).map(|x| buffer[(x, area.y)].symbol()).collect();
    assert!(row.trim_start().starts_with("/beta"), "{row:?}");
    assert!(row.trim_end().ends_with("1 of 1"), "{row:?}");
}

/// Spec 0278 test-plan item 2 (G2, G3, S3). The pattern and the count
/// are one object: the first movement key takes both, and the tally
/// itself is untouched — it is the *row* that moved on, not the search.
#[test]
fn the_echo_and_the_count_leave_together() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    assert!(app.search_row_text().is_some());
    assert!(app.search_tally_text().is_some());

    press(&mut app, KeyCode::Down);
    assert_eq!(app.search_row_text(), None);
    assert_eq!(app.search_tally_text(), None);
    assert!(
        app.search_highlight,
        "the tint outlives the row (spec 0235 S15)"
    );
}

/// Spec 0278 test-plan item 3 (S3's second half). `n` clears the echo
/// with every other keypress and then sets it again, so repeating a
/// search reprints the pattern rather than blanking the row.
#[test]
fn n_reprints_the_pattern() {
    let mut app = sibling_leaves_app(&["beta: 1", "beta: 2", "beta: 3"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    press(&mut app, KeyCode::Down);
    assert_eq!(app.search_row_text(), None);

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.search_row_text().as_deref(), Some("/beta"));
}

/// Spec 0278 test-plan item 4 (S4). A find prompt shows its own prefix
/// while it is open (spec 0276 S3), but what it leaves behind is a
/// committed search and is spelled as one.
#[test]
fn an_accepted_find_echoes_a_committed_search() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    find_by_key(&mut app, 'F', "beta");
    assert_eq!(app.search_row_text().as_deref(), Some(">beta"));

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.search_row_text().as_deref(), Some("/beta"));
}

/// Spec 0278 test-plan item 5 (N3). A miss has `not_found` to say, and
/// the message outranks the echo that is not there.
#[test]
fn a_miss_echoes_nothing() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "nope");

    assert_eq!(app.search_row_text(), None);
    assert!(app.message.contains("not found"), "{}", app.message);
}

/// Spec 0278 test-plan item 6 (N1). The echo is not a message and does
/// not expire: an expired `message_deadline` sweeps the message row and
/// leaves the pattern and its count standing.
#[test]
fn the_echo_outlives_the_message_timeout() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    counted_search(&mut app, "beta");
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    assert_eq!(app.message_deadline, None, "the message row was swept");
    assert_eq!(app.search_row_text().as_deref(), Some("/beta"));
    assert_eq!(app.search_tally_text().as_deref(), Some("1 of 1"));
}

// ---------------------------------------------------------------------
// Spec 0339: one search, three panes.
// ---------------------------------------------------------------------

/// The side pane's cells drawn on `bg`, as `(pane row, first pane
/// column, the symbols in reading order)` — one entry per row that has
/// any.
///
/// Pane columns, not screen columns: `side_area` is the pane's *inner*
/// rect, so its column 0 is the row text's own column 0. A side pane
/// has neither of the main pane's two gutters (spec 0339 S2), which is
/// what makes that identity worth asserting on.
fn side_pane_tint(app: &mut App, bg: Option<Color>) -> Vec<(usize, usize, String)> {
    pane_tint(app, bg, |app| app.side_area)
}

fn pane_tint(
    app: &mut App,
    bg: Option<Color>,
    area_of: impl Fn(&App) -> Rect,
) -> Vec<(usize, usize, String)> {
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let area = area_of(app);
    let buffer = terminal.backend().buffer();
    let mut out = Vec::new();
    for y in area.y..area.y + area.height {
        let mut first = None;
        let mut text = String::new();
        for x in area.x..area.x + area.width {
            let cell = &buffer[(x, y)];
            if cell.style().bg == bg {
                first.get_or_insert((x - area.x) as usize);
                text.push_str(cell.symbol());
            }
        }
        if let Some(column) = first {
            out.push(((y - area.y) as usize, column, text));
        }
    }
    out
}

fn current_tint(app: &mut App) -> Vec<(usize, usize, String)> {
    let bg = theme::search_current_style(app.theme).bg;
    side_pane_tint(app, bg)
}

fn other_tint(app: &mut App) -> Vec<(usize, usize, String)> {
    let bg = theme::search_match_style(app.theme).bg;
    side_pane_tint(app, bg)
}

/// The override pane, open, holding `candidates` and highlighting the
/// first of them.
fn override_pane_app(candidates: &[(&str, Option<i64>)]) -> App {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    press(&mut app, KeyCode::Char('t'));
    assert!(app.override_focus);
    app.override_candidates = candidates
        .iter()
        .map(|(fqdn, score)| ((*fqdn).to_string(), *score))
        .collect();
    app.override_highlight = 0;
    app
}

/// The manage pane, open, holding one entry per given type under a
/// field-scoped origin of its own.
///
/// Field-scoped rather than spec 0117's `Path`, because a `Path`
/// origin's label starts with `/` and so reads as a *path pattern*
/// (spec 0273 S2), which matches no side-pane text at all (N4).
fn manage_pane_app(types: &[&str]) -> App {
    let (mut app, _items) = repeated_message_fixture();
    app.splash = false;
    app.term_width = 120;
    app.manage_open = true;
    app.manage_focus = true;
    for (i, ty) in types.iter().enumerate() {
        app.overrides.activate(
            OverrideOrigin::FqdnField {
                fqdn: "test.Outer".to_string(),
                field: i as u64 + 1,
            },
            Some((*ty).to_string()),
        );
    }
    // Entry 0 is the auto-derived root; the activated ones follow it.
    app.manage_highlight = 0;
    app
}

/// Spec 0339 test-plan item 1 (G1, S4). A `/` in the override pane
/// tints as it is typed: every occurrence in `search_match_style`, and
/// on the entry the sweep is standing on, `search_current_style`.
#[test]
fn a_slash_tints_the_override_pane() {
    let mut app = override_pane_app(&[
        ("pkg.Alpha", None),
        ("pkg.Beta", None),
        ("pkg.Alphabet", None),
    ]);

    type_keys(&mut app, "/Alpha");
    settle_sweep(&mut app);

    // Row 0 is the origin, excluded on the way out (spec 0246 N4), so
    // the sweep stands on row 2.
    assert_eq!(app.search_current_index(), Some(2));
    assert_eq!(current_tint(&mut app), vec![(2, 4, "Alpha".to_string())]);
    assert_eq!(other_tint(&mut app), vec![(0, 4, "Alpha".to_string())]);
    assert_eq!(
        app.override_highlight, 0,
        "a `/` prompt tints without moving the highlight"
    );
}

/// Spec 0339 test-plan item 2 (S3). The tint is a scan of the row as
/// *drawn*, not a projection of the haystack the sweep matched: a
/// candidate carrying an inferred score draws a `  (score: N)` tail
/// that its FQDN haystack knows nothing about, and the tint still lands
/// on the FQDN and stops there.
#[test]
fn the_override_tint_lands_on_the_drawn_text() {
    let mut app = override_pane_app(&[("pkg.Alpha", Some(7))]);
    assert_eq!(app.override_row_display(0).0, "pkg.Alpha  (score: 7)");

    type_keys(&mut app, "/Alpha");
    settle_sweep(&mut app);

    assert_eq!(current_tint(&mut app), vec![(0, 4, "Alpha".to_string())]);
}

/// Spec 0339 test-plan item 3 (S3, S4). The manage pane tints its type
/// label, at the column the marker prefix puts it at — which is not
/// where the same text sits in `manage_search_text`.
#[test]
fn a_slash_tints_the_manage_pane() {
    let mut app = manage_pane_app(&["pkg.zz1", "pkg.zz2"]);

    type_keys(&mut app, "/zz2");
    settle_sweep(&mut app);

    assert_eq!(app.search_current_index(), Some(2));
    // Entry 2 draws at pane row 5: each of the three entries has its own
    // origin, so each is preceded by its own header row.
    //
    // `  ● pkg.zz2` — the marker and its two spaces put the label at
    // column 4, so `zz2` lands at column 8. The haystack, meanwhile, has
    // the origin label in front of the label and no marker at all; only
    // a re-scan of the drawn row can put the tint here.
    assert!(app.manage_search_text(2).starts_with("test.Outer:2 "));
    assert_eq!(current_tint(&mut app), vec![(5, 8, "zz2".to_string())]);
}

/// Spec 0339 test-plan item 3, second half (S7, N1). The origin label
/// is in the haystack but is drawn on the *header* row above the entry,
/// and a header is not a candidate — so an origin-only match tints
/// nothing at all while still landing the highlight on the entry.
#[test]
fn an_origin_only_match_lands_without_tinting() {
    let mut app = manage_pane_app(&["pkg.zz1", "pkg.zz2"]);

    commit_search_by_key(&mut app, "Outer:2");

    assert_eq!(app.manage_highlight, 2, "{}", app.message);
    assert_eq!(current_tint(&mut app), Vec::new());
    assert_eq!(other_tint(&mut app), Vec::new());
}

/// Spec 0339 test-plan item 4 (S7). The `as "x"` a row draws is part of
/// what the pane searches.
#[test]
fn the_manage_display_name_is_searchable() {
    let mut app = manage_pane_app(&["pkg.zz1", "pkg.zz2"]);
    app.overrides.rename(1, Some("zebra".to_string()));
    assert!(app.manage_type_line(1).contains("as \"zebra\""));

    commit_search_by_key(&mut app, "zebra");

    assert_eq!(app.manage_highlight, 1, "{}", app.message);
}

/// Spec 0339 test-plan item 5 (N1). A pattern matching only the origin
/// labels stops once per matching *entry* over a full `n` cycle — a
/// header row is drawn, and searched through the entries it groups, but
/// is never itself a landing.
#[test]
fn a_manage_header_is_never_a_landing() {
    let mut app = manage_pane_app(&["pkg.zz1", "pkg.zz2", "pkg.zz3"]);

    // The root entry's origin is `/`, so only the three field-scoped
    // ones carry a `test.Outer:` label.
    commit_search_by_key(&mut app, "Outer:");
    let first = app.manage_highlight;

    let mut stops = vec![first];
    loop {
        press(&mut app, KeyCode::Char('n'));
        if app.manage_highlight == first {
            break;
        }
        stops.push(app.manage_highlight);
        assert!(stops.len() <= 8, "a cycle must close: {stops:?}");
    }
    assert_eq!(stops, vec![1, 2, 3]);
}

/// Spec 0339 test-plan item 6 (S6, N2). A side pane's entries are a
/// list, never one joined haystack: a pattern reaching from the end of
/// one entry into the next finds nothing, while the same shape of
/// pattern inside a single entry finds it.
#[test]
fn no_match_crosses_two_entries() {
    let mut app = override_pane_app(&[("pkg.Alpha", None), ("pkg.Beta", None)]);

    commit_search_by_key(&mut app, r"Alpha\s+pkg");
    assert!(app.message.contains("not found"), "{}", app.message);

    // The same engine — `\s*` admits a newline, so this compiles to
    // `SearchPattern::Multi` too — matching within one entry.
    commit_search_by_key(&mut app, r"Alpha\s*");
    assert_eq!(app.search_current_index(), Some(0), "{}", app.message);
}

/// Spec 0339 test-plan item 7. `Esc` still puts a side pane back, now
/// that a `/` visibly changes it: the view returns and the tint goes
/// with the prompt.
#[test]
fn esc_still_restores_a_side_pane() {
    let mut app = override_pane_app(&[
        ("pkg.Alpha", None),
        ("pkg.Beta", None),
        ("pkg.Alphabet", None),
    ]);
    let scroll = app.override_scroll;
    app.override_pan_offset = 2;

    type_keys(&mut app, "/Alpha");
    settle_sweep(&mut app);
    assert!(!current_tint(&mut app).is_empty(), "the prompt tinted");

    press(&mut app, KeyCode::Esc);
    assert!(app.command_buffer.is_none());
    assert_eq!(app.override_highlight, 0);
    assert_eq!(app.override_scroll, scroll);
    assert_eq!(app.override_pan_offset, 2);
    assert_eq!(current_tint(&mut app), Vec::new());
    assert_eq!(other_tint(&mut app), Vec::new());
}

/// Spec 0339 test-plan item 8 (S9). One `repeat_search`, three
/// independent memories: `n` in a pane repeats that pane's own last
/// committed pattern and leaves the other two alone.
#[test]
fn n_repeats_its_own_panes_search() {
    let mut app = override_pane_app(&[
        ("pkg.Alpha", None),
        ("pkg.Beta", None),
        ("pkg.Alphabet", None),
    ]);
    app.set_last_search_for(
        SearchScope::Main,
        (SearchDir::Forward, "main-only".to_string()),
    );

    commit_search_by_key(&mut app, "Alpha");
    assert_eq!(app.override_highlight, 2);

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.override_highlight, 0, "{}", app.message);
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        Some(&(SearchDir::Forward, "main-only".to_string())),
        "the main pane's memory is untouched"
    );
    assert_eq!(app.last_search_for(SearchScope::Manage), None);

    let mut app = manage_pane_app(&["pkg.zz1", "pkg.zz2", "pkg.zz3"]);
    app.set_last_search_for(
        SearchScope::Override,
        (SearchDir::Forward, "override-only".to_string()),
    );

    commit_search_by_key(&mut app, "zz");
    assert_eq!(app.manage_highlight, 1);

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.manage_highlight, 2, "{}", app.message);
    assert_eq!(
        app.last_search_for(SearchScope::Override),
        Some(&(SearchDir::Forward, "override-only".to_string())),
        "the override pane's memory is untouched"
    );
}

// ---------------------------------------------------------------------
// Spec 0340: the help overlay is a pane.
// ---------------------------------------------------------------------

fn help_current_tint(app: &mut App) -> Vec<(usize, usize, String)> {
    let bg = theme::search_current_style(app.theme).bg;
    pane_tint(app, bg, |app| app.help_area)
}

fn help_other_tint(app: &mut App) -> Vec<(usize, usize, String)> {
    let bg = theme::search_match_style(app.theme).bg;
    pane_tint(app, bg, |app| app.help_area)
}

/// The `F1` overlay, open and drawn once so that `help_area` and
/// `help_list_height` say what a live frame would.
fn help_app() -> App {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    press(&mut app, KeyCode::F(1));
    assert!(app.help_open);
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    app
}

/// A substring the help uses on two lines close enough together that one
/// window holds both — the shape the tint test needs, and the one the
/// help's two mirrored `h`/`l` entries happen to give it.
const TWICE: &str = "caret one character";

/// The rows `TWICE` falls on, and the column it starts at on each. Both
/// derived from `HELP_TEXT` rather than written down, so that an edit to
/// the help moves the expectation with it instead of breaking it.
fn twice_hits() -> Vec<(usize, usize)> {
    HELP_TEXT
        .iter()
        .enumerate()
        .filter_map(|(i, l)| l.find(TWICE).map(|c| (i, c)))
        .collect()
}

/// Spec 0340 test-plan item 4 (S6, S7). The overlay tints as the pattern
/// is typed: the line the sweep is standing on in the current style,
/// another line holding the same substring in the other.
#[test]
fn a_slash_tints_the_help_overlay() {
    let hits = twice_hits();
    let mut app = help_app();
    // The first two hits must share the drawn window — one to be the
    // current match, one to be an "other" — and every later hit must
    // fall off it, or it would show up in the expectations below.
    let drawn = app.help_list_height;
    assert!(
        hits.len() >= 2 && hits[1].0 < drawn && hits[2..].iter().all(|(i, _)| *i >= drawn),
        "the fixture wants two hits in the first {drawn} rows and none after: {hits:?}"
    );

    type_keys(&mut app, "/");
    type_keys(&mut app, TWICE);
    settle_sweep(&mut app);

    assert_eq!(app.search_current_index(), Some(hits[0].0));
    assert_eq!(
        app.help_highlight, 0,
        "a `/` tints without moving the cursor — that is what the tint is for"
    );
    // Pane rows, and the line's own columns: the overlay is unscrolled
    // and unpanned, and unlike either side pane its haystack is the very
    // text it draws (S6), so the two coincide exactly.
    assert_eq!(
        help_current_tint(&mut app),
        vec![(hits[0].0, hits[0].1, TWICE.to_string())]
    );
    assert_eq!(
        help_other_tint(&mut app),
        vec![(hits[1].0, hits[1].1, TWICE.to_string())]
    );
}

/// Spec 0340 test-plan item 7 (S1, S4). A committed search lands the
/// overlay's *cursor*, and `n` carries on from there.
#[test]
fn the_help_cursor_is_the_search_landing() {
    let hits = twice_hits();
    let mut app = help_app();

    commit_search_by_key(&mut app, TWICE);
    assert_eq!(app.help_highlight, hits[0].0, "{}", app.message);

    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.help_highlight, hits[1].0, "{}", app.message);
}

/// Spec 0340 test-plan item 5 (S9). The overlay's last pattern is its
/// own: `n` there repeats it and leaves the other three panes' alone.
#[test]
fn n_in_the_help_repeats_only_its_own_search() {
    let mut app = help_app();
    app.set_last_search_for(
        SearchScope::Main,
        (SearchDir::Forward, "main-only".to_string()),
    );

    commit_search_by_key(&mut app, TWICE);
    let landed = app.help_highlight;
    press(&mut app, KeyCode::Char('n'));
    assert_ne!(app.help_highlight, landed, "{}", app.message);

    assert_eq!(
        app.last_search_for(SearchScope::Help),
        Some(&(SearchDir::Forward, TWICE.to_string()))
    );
    assert_eq!(
        app.last_search_for(SearchScope::Main),
        Some(&(SearchDir::Forward, "main-only".to_string())),
        "the main pane's memory is untouched"
    );
    assert_eq!(app.last_search_for(SearchScope::Override), None);
}

/// Spec 0340 test-plan item 6 (S1). `Esc` puts the overlay back where
/// the prompt found it — which is only expressible now that its scroll
/// is a `PaneScroll` and not a bare offset.
#[test]
fn esc_restores_the_help_view() {
    let mut app = help_app();
    app.help_pan_offset = 3;
    let scroll = app.help_scroll;

    // A pattern near the bottom of the help, so that previewing it has
    // to scroll — otherwise there is nothing for `Esc` to restore.
    type_keys(&mut app, "/suspend");
    settle_sweep(&mut app);
    assert!(!help_current_tint(&mut app).is_empty(), "the prompt tinted");
    assert_ne!(app.help_scroll, scroll, "and had to scroll to show it");

    press(&mut app, KeyCode::Esc);
    assert!(app.command_buffer.is_none());
    assert_eq!(app.help_highlight, 0, "the cursor never moved");
    assert_eq!(app.help_scroll, scroll);
    assert_eq!(app.help_pan_offset, 3);
    assert_eq!(help_current_tint(&mut app), Vec::new());
    assert_eq!(help_other_tint(&mut app), Vec::new());
}

/// Spec 0340 N4. The `\n` between two help lines is an artifact of the
/// drawing, so no pattern reads across it — while the same pattern
/// inside one line matches.
#[test]
fn no_match_crosses_two_help_lines() {
    let mut app = help_app();
    assert_eq!(HELP_TEXT[0], "protolens — key bindings");

    commit_search_by_key(&mut app, r"bindings\s+Movement");
    assert_eq!(app.help_highlight, 0, "nothing to land on");
    assert!(app.message.contains("not found"), "{}", app.message);

    app.message.clear();
    commit_search_by_key(&mut app, r"key\s+bindings");
    assert_eq!(app.help_highlight, 0, "{}", app.message);
    assert!(app.message.is_empty(), "{}", app.message);
}
