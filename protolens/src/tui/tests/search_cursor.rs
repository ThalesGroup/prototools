// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0274: the document read as one string, and searched that way.
//!
//! The tests here are about the haystack rather than about the prompt:
//! that walking the arena as chunks reproduces exactly the rows the line
//! walk draws, that a bake hole is a boundary a match cannot cross, and
//! that a superseded search stops at the next node rather than at the
//! next byte.

use super::super::search_cursor::{DocCursor, Segment};
use super::super::*;
use super::support::*;
use regex_cursor::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

/// The rows the line walk draws, joined — the string spec 0274 S1 says
/// a non-path pattern searches, built the slow and obvious way so that
/// the cursor has something independent to agree with.
fn document(app: &App) -> String {
    let mut out = String::new();
    let mut at = app.first_line();
    while let Some(pos) = at {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&app.line_text(pos));
        at = app.next_line(pos);
    }
    out
}

/// The bytes [`DocCursor`] hands out for `seg`, concatenated.
fn cursor_text(app: &App, seg: Segment) -> String {
    let mut cursor = DocCursor::new(app.structure(), &app.node_text, seg, None);
    let mut out = Vec::new();
    loop {
        out.extend_from_slice(cursor.chunk());
        if !cursor.advance() {
            break;
        }
    }
    String::from_utf8(out).expect("chunks never split a codepoint")
}

/// Spec 0274 S6. The chunk walk is the line walk: same rows, same
/// separators, no allocation of the whole.
///
/// This is the property everything else rests on — an offset the engine
/// reports is only meaningful if the bytes it counted are the bytes the
/// reader sees.
#[test]
fn the_cursor_hands_out_the_rows_joined() {
    let app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    let (segments, stops) = app.search_segments();
    assert!(stops.is_empty(), "a hand-built document has no bake stops");
    assert_eq!(segments.len(), 1, "and so exactly one segment");
    assert_eq!(cursor_text(&app, segments[0]), document(&app));
    assert_eq!(document(&app), "alpha: 1\nbeta: 2\ngamma: 3");
}

/// Spec 0274 S6: `backtrack` undoes `advance` step for step, which is
/// what lets the engine look behind a chunk seam.
///
/// regex-cursor requires it, and it is derived rather than stored:
/// `Place::prev` is `Place::next` read the other way over the same
/// level-ordered arena (spec 0216).
#[test]
fn backtracking_retraces_the_walk() {
    let app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    let (segments, _) = app.search_segments();
    let mut cursor = DocCursor::new(app.structure(), &app.node_text, segments[0], None);

    let mut forward = vec![(cursor.offset(), cursor.chunk().to_vec())];
    while cursor.advance() {
        forward.push((cursor.offset(), cursor.chunk().to_vec()));
    }
    let mut backward = vec![(cursor.offset(), cursor.chunk().to_vec())];
    while cursor.backtrack() {
        backward.push((cursor.offset(), cursor.chunk().to_vec()));
    }
    backward.reverse();
    assert_eq!(forward, backward);
}

/// Spec 0274 test-plan item 3 (G1, S1). `id\nvalue` compiles — 0273
/// refused it — and lands on the row the match *starts* on.
#[test]
fn a_match_across_two_rows_is_found_at_the_right_place() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\nbeta");
    assert!(app.message.is_empty(), "{}", app.message);
    assert_eq!(app.cursor, 0, "the match starts on alpha's row");
    // `alpha: 1` — the `1` is column 7.
    assert_eq!(app.cursor_column, 7);

    // The same pattern with a `\n` the rows do not have finds nothing.
    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\ngamma");
    assert!(app.message.contains("not found"), "{}", app.message);
}

/// Spec 0274 S2 as a *semantics* rather than a routing rule: a pattern
/// that admits no newline takes the per-row walk and must agree with the
/// one that does. `beta` and `\s*beta` are the same search.
#[test]
fn the_two_engines_agree_where_the_pattern_admits_no_newline() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    for pattern in ["beta", "beta|zzz", "b[e]ta"] {
        app.run_search(SearchScope::Main, SearchDir::Forward, pattern);
        assert!(app.message.is_empty(), "{pattern}: {}", app.message);
        assert_eq!(app.cursor, 1, "{pattern}");
        assert_eq!(app.cursor_column, 0, "{pattern}");
    }
}

/// Spec 0274 test-plan items 4 and 5 (S4). A bake stop's body has not
/// been rendered, so the row after its header is not its successor: a
/// match must not be allowed to join them, and reaching the hole is not
/// the end of the search.
#[test]
fn a_match_does_not_cross_a_bake_hole_but_survives_it() {
    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;
    assert!(
        !app.auto_folded.is_empty(),
        "a bounded open must leave stops"
    );

    let (segments, stops) = app.search_segments();
    assert_eq!(
        segments.len(),
        stops.len() + 1,
        "every stop closes a segment and opens the next"
    );
    // Each segment on its own is a run of rows that really are adjacent,
    // and together they are the document minus its seams.
    let joined: Vec<String> = segments.iter().map(|&s| cursor_text(&app, s)).collect();
    let whole = document(&app);
    for part in &joined {
        assert!(whole.contains(part.as_str()), "{part:?} is not a run");
    }

    // A pattern spanning a seam misses, and says so provisionally
    // because the bake still owes the document text (S15).
    let seam = format!(
        "{}\n{}",
        joined[0].lines().next_back().expect("a first segment"),
        joined[1].lines().next().expect("a second segment"),
    );
    app.run_search(SearchScope::Main, SearchDir::Forward, &regex_quote(&seam));
    assert!(app.message.contains("not yet baked"), "{}", app.message);

    // The same search wholly inside the far side of the hole is found:
    // a hole ends a haystack, not the walk.
    let far = joined
        .last()
        .expect("a last segment")
        .lines()
        .next_back()
        .expect("a last row")
        .trim()
        .to_string();
    app.message.clear();
    app.run_search(
        SearchScope::Main,
        SearchDir::Forward,
        &format!(r"{}\s*", regex_quote(&far)),
    );
    assert!(app.message.is_empty(), "{}", app.message);
}

/// Escape a rendered row so it can be used as a literal inside a
/// pattern that also carries a `\n`. Only the metacharacters these
/// fixtures actually produce need handling.
fn regex_quote(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            out.push_str("\\n");
            continue;
        }
        if "\\.+*?()|[]{}^$".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Spec 0274 test-plan item 10 (S7). The cursor is the abort point: a
/// search whose epoch has moved on ends at the next chunk boundary, the
/// same way one that ran out of data does.
///
/// The check is one relaxed load per node rather than per byte, which is
/// why it is `advance` that carries it and not the engine.
#[test]
fn an_aborted_search_stops_at_the_next_chunk() {
    let app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    let (segments, _) = app.search_segments();
    let epoch = AtomicU64::new(7);

    let mut cursor = DocCursor::new(
        app.structure(),
        &app.node_text,
        segments[0],
        Some((&epoch, 7)),
    );
    assert!(cursor.advance(), "the epoch still holds");
    epoch.store(8, Ordering::Relaxed);
    assert!(!cursor.advance(), "a superseded search ends here");
    // And ends *cleanly*: the chunk it stopped on is still the one it
    // was showing, so nothing the engine already read is contradicted.
    assert_eq!(cursor.chunk(), b"\n");
}
