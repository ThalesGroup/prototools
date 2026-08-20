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
use std::sync::mpsc;
use std::time::Duration;

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

/// Spec 0274 test-plan item 3 (G1, S1) and S12. `id\nvalue` compiles —
/// 0273 refused it — and comes back selected from the row it starts on
/// to the row it ends on.
#[test]
fn a_match_across_two_rows_is_found_at_the_right_place() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\nbeta");
    assert!(app.message.is_empty(), "{}", app.message);
    // `alpha: 1` — the `1` is column 7, and a search leaves the caret
    // where the match begins, so that is where the caret rests.
    assert_eq!(app.cursor, 0, "the match starts on alpha's row");
    assert_eq!(app.cursor_column, 7);
    // The anchor is the fixed end, on the match's last character — the
    // `a` of `beta`.
    let anchor = app.select_anchor.expect("a match over two rows selects");
    assert_eq!((anchor.node, anchor.column), (1, 3));
    assert!(app.select_engaged);

    // The same pattern with a `\n` the rows do not have finds nothing.
    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\ngamma");
    assert!(app.message.contains("not found"), "{}", app.message);
}

/// Spec 0274 test-plan item 7 (S12). The multi-row result the request
/// was about: `Enter` leaves the match selected, expressed as spec
/// 0242's ordinary selection — so `selected_text` gives back exactly
/// the bytes the pattern matched, and `Ctrl-c` learns nothing.
#[test]
fn a_multi_row_hit_becomes_a_selection() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
    app.splash = false;
    app.term_width = 120;

    open_search(&mut app, r"1\nbeta");
    settle(&mut app);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // `alpha: 1` from column 7 to `beta: 2` up to column 4, the end
    // exclusive — the whole of the match and nothing besides.
    assert_eq!(app.selection_span(), Some((0, 7, 1, 4)));
    assert_eq!(app.selected_text(), Some((2, "1\nbeta".to_string())));
}

/// Spec 0274 test-plan item 8 (S12's second half). A hit inside one row
/// selects nothing, whichever engine found it — a selection there would
/// be a cue the reader did not ask for, over text one glance takes in.
#[test]
fn a_single_row_hit_still_sets_no_selection() {
    // `b\s*eta` admits a `\n` and so routes to the cross-row engine,
    // while `beta` does not: the two paths must agree about this.
    // Deliberately not `\s*beta`, which is *not* a single-row match —
    // `\s*` is greedy and eats the newline the previous row ends with.
    for pattern in [r"b\s*eta", "beta"] {
        let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3"]);
        app.splash = false;
        app.term_width = 120;

        open_search(&mut app, pattern);
        settle(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.cursor, 1, "{pattern}");
        assert_eq!(app.select_anchor, None, "{pattern}");
        assert_eq!(app.selection_span(), None, "{pattern}");
    }
}

/// Spec 0274 S12. `N` after `/` steps *off* the cross-row match it is
/// standing on, and does not find it again.
///
/// The rule this pins is where a search leaves the caret. Spec 0246 S3
/// splits the origin at the caret and relies on the two halves to
/// partition it; a caret parked at the match's *end* leaves the match's
/// own start inside the backward half, so `N` re-finds it and the reader
/// sees a key that does nothing. Single-row search never had the problem
/// because its caret lands on the match's first character — this is the
/// cross-row engine keeping the same invariant.
#[test]
fn a_backward_search_steps_off_the_cross_row_match_it_is_on() {
    let mut app = sibling_leaves_app(&["a: 1", "b: 2", "a: 1", "b: 2", "c: 3"]);
    app.splash = false;
    app.term_width = 120;

    // Forward twice, to the second occurrence: node 2's row into node
    // 3's.
    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\nb");
    app.run_search(SearchScope::Main, SearchDir::Forward, r"1\nb");
    assert_eq!(app.cursor, 2, "{}", app.message);

    // And back to the first, on node 0 — not round to itself.
    app.run_search(SearchScope::Main, SearchDir::Backward, r"1\nb");
    assert_eq!(app.cursor, 0, "{}", app.message);
}

/// Spec 0274 S2 as a *semantics* rather than a routing rule: a pattern
/// that admits no newline takes the per-row walk and must agree with the
/// one that does. `beta` and `b\s*eta` are the same search.
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

/// Spec 0274 S13. The window-wide highlight pass is cut where the drawn
/// rows are not document-adjacent, so it agrees with the sweep about
/// which `\n` a pattern may match.
///
/// A bake hole is the case the line numbers cannot catch on their own:
/// an unbaked stop draws its header and its footer on *consecutive*
/// absolute lines with its whole subtree still missing between them. Get
/// this wrong and the reader is shown a match that is not there — and
/// pressing `n` for it finds nothing, since the sweep does not believe
/// in it either.
#[test]
fn the_window_highlight_does_not_join_rows_across_a_bake_hole() {
    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;

    // The document opens `1 {` and then draws one folded `items` stop
    // per row: row 0 is ordinary, rows 1 and 2 are both stop headers.
    let window = app.build_window(0, 4);
    // The rows as *drawn*, which is the haystack the highlight uses and
    // is not `line_text`: a folded stop's header is drawn `{ ... }`.
    let drawn: Vec<String> = window.iter().map(|&row| app.row_text(row)).collect();
    let stop_at = |row| match window[row] {
        DisplayRow::Committed(c) => app.auto_folded.contains(c.pos.node) && !app.is_footer(c.pos),
        DisplayRow::Overlay(_) => false,
    };
    assert!(!stop_at(0) && stop_at(1) && stop_at(2), "{drawn:?}");

    let occurrences = |pair: String| {
        let pattern = SearchPattern::new(&regex_quote(&pair)).expect("a literal pair compiles");
        app.multi_row_occurrences(&pattern, 0, &window)
            .into_iter()
            .filter(|row| !row.is_empty())
            .count()
    };

    // The control: two rows that really are adjacent are found, so the
    // negative below cannot pass by finding nothing at all.
    let inside = format!("{}\n{}", drawn[0], drawn[1]);
    assert_eq!(occurrences(inside), 2, "an adjacency is a match");

    // The seam: two rows drawn one above the other, on *consecutive*
    // absolute lines — the stop's unrendered body is what stands between
    // them, and the line numbers do not show it.
    let seam = format!("{}\n{}", drawn[1], drawn[2]);
    assert_eq!(occurrences(seam), 0, "a hole is not a newline");
}

/// Open a `/` prompt and type `pattern` into it, which is what arms a
/// live sweep — `run_search` drains one instead, and the states this
/// file is about are the ones a sweep passes through.
fn open_search(app: &mut App, pattern: &str) {
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in pattern.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

/// Step the live sweep the way `run_loop`'s idle arm does, until it has
/// nothing left to say.
///
/// Sound only where `search_progress` is `None`: with a worker in play
/// an `Idle` is a yield rather than an ending, which is what
/// [`a_segment_scan_waits_for_its_worker_and_then_yields`] is about.
fn settle(app: &mut App) {
    for _ in 0..10_000 {
        if app.search_sweep_step() == SweepStep::Idle {
            return;
        }
    }
    panic!("a sweep must converge");
}

/// Run the bake to exhaustion, as `tests::bake::drain` does.
fn bake(app: &mut App) {
    for _ in 0..10_000 {
        if app.bake_step() == BakeStep::Idle {
            return;
        }
    }
    panic!("a bake must terminate");
}

/// Spec 0274 test-plan items 11 and 12 (S9, S10). The two halves of the
/// worker's contract with `run_loop`'s idle arm, which are only
/// meaningful together.
///
/// While a segment is out with a worker the step reports `Waiting`, and
/// the idle arm reads that as "skip discard, bake and read-ahead" — a
/// bake step would have to stop the scan to write the document, and the
/// two heat jobs would take the core it is running on. But the step that
/// *collects* a verdict reports `Idle`, and that is the pass on which
/// those three each get their turn. Report `Progressed` there instead
/// and the arm `continue`s past all of them for the length of the
/// search, which is the regression this pins.
#[test]
fn a_segment_scan_waits_for_its_worker_and_then_yields() {
    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;
    let (segments, _) = app.search_segments();
    assert!(segments.len() > 1, "the fixture must have several segments");

    // Spec 0274 S9: a sender is what makes a scan offloadable, so this
    // stands in for `run()` having a loop to report to.
    let (tx, rx) = mpsc::channel();
    app.search_progress = Some(tx);
    open_search(&mut app, r"zzqq\nxxww");

    let mut handed_out = 0usize;
    let mut yields_with_work_left = 0usize;
    for _ in 0..10_000 {
        match app.search_sweep_step() {
            SweepStep::Waiting => {
                handed_out += 1;
                // The wake-up the idle arm would have slept on. It
                // arrives *after* the answer, so the next step collects
                // rather than waiting again.
                rx.recv_timeout(Duration::from_secs(10))
                    .expect("a scan must report");
            }
            SweepStep::Idle if app.search_segment_pending() => yields_with_work_left += 1,
            SweepStep::Idle => break,
            SweepStep::Progressed => panic!("an offloaded segment must not be scanned inline"),
        }
    }
    // One task per queue entry, and S14's queue is one longer than the
    // segmentation: 0246 S2's two visits to the origin's own segment,
    // split at the caret.
    assert_eq!(
        handed_out,
        segments.len() + 1,
        "one worker per queued segment, and no segment skipped"
    );
    assert_eq!(
        yields_with_work_left,
        segments.len(),
        "every collect but the last must let the other three jobs run"
    );
    assert!(
        app.search_sweep.as_ref().expect("a sweep").found.is_none(),
        "the pattern is not in the fixture"
    );
}

/// Spec 0274 test-plan item 13 (S9, S15), and item 6 with it. The queue
/// is frozen at search start, so a segment the bake creates does not
/// join the walk in progress — the answer is a *provisional* miss, and
/// the second pass is what revises it.
///
/// The alternative the user rejected is what this forbids: honoring a
/// join means rescanning both sides of it, a join lands every ~22 ms,
/// and each is larger than the last.
#[test]
fn a_segment_the_bake_creates_does_not_join_the_queue() {
    // The row pair to look for is taken from a finished copy of the same
    // fixture: two rows that are adjacent only once the bake has
    // rendered the stop's body, so nothing in the document as it stands
    // can match.
    let mut finished = bounded_repeated_message_fixture(2).0;
    bake(&mut finished);
    let whole = document(&finished);
    let rows: Vec<&str> = whole.lines().collect();

    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;
    let started_as = document(&app);
    let target = rows
        .windows(2)
        .map(|w| format!("{}\n{}", w[0], w[1]))
        .find(|pair| !started_as.contains(pair.as_str()))
        .expect("the bake must add an adjacency");
    let pattern = regex_quote(&target);

    // Pass one, over the document as it stands. A miss, and the report
    // says it is not an answer yet.
    open_search(&mut app, &pattern);
    settle(&mut app);
    assert!(app.search_sweep.as_ref().expect("a sweep").found.is_none());
    assert!(
        app.not_found(&pattern, SearchScope::Main)
            .contains("not yet baked"),
        "{}",
        app.not_found(&pattern, SearchScope::Main)
    );

    // The bake finishes the document underneath the finished sweep. A
    // frozen queue means it grew by nothing.
    bake(&mut app);
    assert!(app.auto_folded.is_empty());
    assert!(
        app.search_sweep.as_ref().expect("a sweep").found.is_none(),
        "a bake step must not make a finished sweep find anything"
    );

    // Pass two: the first step after the last subtree landed asks the
    // question again, and this time the adjacency is there.
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    settle(&mut app);
    assert!(
        app.search_sweep.as_ref().expect("a sweep").found.is_some(),
        "the second pass must find what the first could not reach"
    );
}

/// Spec 0274 test-plan item 14 (S16). `Ctrl-Right` on a *provisional*
/// miss begins a fresh sweep; on a conclusive one it stays 0246 S21's
/// no-op.
///
/// Without the first half the reader is stuck at exactly the moment the
/// gesture is for: the prompt says "not there yet" and the key that
/// means "ask again" does nothing. Without the second, a search that has
/// genuinely failed would re-walk the whole document on every press.
#[test]
fn a_provisional_miss_rotates_and_a_conclusive_one_does_not() {
    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;
    open_search(&mut app, r"zzqq\nxxww");
    settle(&mut app);
    assert!(app.search_sweep.as_ref().expect("a sweep").is_finished());
    assert!(app
        .not_found("zzqq", SearchScope::Main)
        .contains("not yet baked"));

    app.rotate_search_match(SearchDir::Forward);
    assert!(
        !app.search_sweep.as_ref().expect("a sweep").is_finished(),
        "a provisional miss must re-segment and walk again"
    );

    // The same pattern over a document with nothing left unread.
    let (mut app, _) = bounded_repeated_message_fixture(2);
    app.splash = false;
    app.term_width = 120;
    bake(&mut app);
    open_search(&mut app, r"zzqq\nxxww");
    settle(&mut app);
    assert!(app
        .not_found("zzqq", SearchScope::Main)
        .contains("not found"));

    app.rotate_search_match(SearchDir::Forward);
    assert!(
        app.search_sweep.as_ref().expect("a sweep").is_finished(),
        "there is nothing left to ask, so the key does nothing"
    );
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
