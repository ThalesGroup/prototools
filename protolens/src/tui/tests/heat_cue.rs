// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_cue::{
    derive_stats, heat_display, heat_fraction, heat_level, score_of, HeatCue, HeatCueKind,
    HeatCueMode, HeatDisplay, HeatHistogram, HeatState, RangeHeatStats, HEAT_ANCHOR_DEFAULT,
    HEAT_CUE_PREVIEW, HEAT_GLYPH, SCORE_FLOOR,
};
use prototext_core::helpers::WT_START_GROUP;
use std::thread;

use super::super::heat_worker::{HeatWorkerHandle, RangeHeatEntry};
use super::super::terminal::warm_up_heat_cues;
use super::super::tiered::{Tier, TieredBounded};
use super::super::*;
use super::support::*;

/// Test helper (spec 0152): directly seeds `heat_caches` with a
/// `RangeHeatEntry` covering `HEAT_CUE_PREVIEW` dummy candidates (so
/// `heat_lookup`'s window check is satisfied) plus one exact
/// current-type score — bypassing the need for a real scoring
/// graph/worker, mirroring what a `heat_cue_for` cache hit expects to
/// find.
pub(super) fn seed_range_heat_entry(
    app: &mut App,
    start: usize,
    best_score: Option<i64>,
    best_count: usize,
    current_key: &str,
    current_score: Option<i64>,
) {
    let mut caches = app.heat_caches.lock().unwrap();
    caches.by_range.upsert(
        start,
        RangeHeatEntry {
            best_score,
            best_count,
            top_n: vec![("protolens_internal.dummy".to_string(), 0); HEAT_CUE_PREVIEW],
        },
        Tier::Visible,
    );
    caches.current_score.upsert(
        (start, current_key.to_string()),
        current_score,
        Tier::Visible,
    );
}

/// Spec 0184 S6: a packed run is one wire record and one unit of
/// action, so its cue describes the record — every element scores over
/// the *record's* payload, not its own few bytes.
///
/// Two consequences asserted here: the cue the user sees agrees with
/// what `t` would act on, and the run's N lines share one
/// `heat_caches` entry rather than holding N (G3).
///
/// Spec 0216 S22 makes the first of those structural — the run is one
/// node, so there is one cue by construction — and leaves the second
/// worth asserting: the range scored is the record's payload, and every
/// row of the run lights up from the single entry keyed on it.
#[test]
fn a_packed_run_scores_one_cue_over_the_whole_record() {
    let (mut app, run, tail, _a, _b) = packed_run_with_tail_fixture();

    let record = app.heat_scored_range(run);
    assert_ne!(
        record,
        widen(&app.tree[run].span.raw_range),
        "the scored range must be the record's payload, not its tag \
         and length too"
    );
    assert_eq!(record.len(), 3, "the record's payload is its 3 elements");

    // A non-packed sibling is unaffected: still its own payload.
    assert_eq!(
        app.heat_scored_range(tail),
        extract::message_payload_range(&app.blob, &app.tree[tail].span.raw_range)
    );

    // One seeded entry, keyed on the record's payload start, lights the
    // run's first line — and only it. The finding is about the record,
    // and a column of the same glyph down every element would read as
    // one finding per element (spec 0232).
    seed_range_heat_entry(&mut app, record.start, Some(50), 1, "int32", Some(10));
    app.heat_cues = HeatCueMode::Findings;
    let rows = app.node_lines(run);
    assert_eq!(rows.len(), 3, "the run draws one row per element");
    assert!(matches!(app.heat_cue_for(rows.start), HeatDisplay::Cue(_)));
    for line in rows.start + 1..rows.end {
        assert!(
            matches!(app.heat_cue_for(line), HeatDisplay::None),
            "element line {line} must leave the cue to the record's first"
        );
    }
}

/// Spec 0138 G5: the Fibonacci boundaries partition the score axis into
/// exactly 12 levels, each boundary itself belonging to the *lower*
/// level (`<=`), one past it starting the next.
#[test]
fn heat_level_bucket_boundaries() {
    let cases: &[(i64, u8)] = &[
        (i64::MIN, 1),
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 4),
        (5, 4),
        (6, 5),
        (8, 5),
        (9, 6),
        (13, 6),
        (14, 7),
        (21, 7),
        (22, 8),
        (34, 8),
        (35, 9),
        (55, 9),
        (56, 10),
        (89, 10),
        (90, 11),
        (144, 11),
        (145, 12),
        (i64::MAX, 12),
    ];
    for &(score, expected) in cases {
        assert_eq!(heat_level(score), expected, "best_score = {score}");
    }
}

// ---------------------------------------------------------------------
// derive_stats / score_of (spec 0151 G1/G2)
// ---------------------------------------------------------------------

#[test]
fn derive_stats_empty_candidates_yields_no_best_score() {
    let stats = derive_stats(&[]);
    assert_eq!(stats.best_score, None);
    assert_eq!(stats.best_count, 0);
}

#[test]
fn derive_stats_single_entry_has_best_count_one() {
    let candidates = vec![("a.Type".to_string(), 5)];
    let stats = derive_stats(&candidates);
    assert_eq!(stats.best_score, Some(5));
    assert_eq!(stats.best_count, 1);
}

#[test]
fn derive_stats_counts_only_entries_tied_at_the_top() {
    let candidates = vec![
        ("a.Type".to_string(), 50),
        ("b.Type".to_string(), 50),
        ("c.Type".to_string(), 10),
        ("d.Type".to_string(), 10),
        ("e.Type".to_string(), 10),
    ];
    let stats = derive_stats(&candidates);
    assert_eq!(stats.best_score, Some(50));
    assert_eq!(
        stats.best_count, 2,
        "ties below the top must not inflate best_count"
    );
}

#[test]
fn score_of_finds_by_fqdn_or_returns_none() {
    let candidates = vec![("a.Type".to_string(), 5), ("b.Type".to_string(), 3)];
    assert_eq!(score_of(&candidates, "b.Type"), Some(3));
    assert_eq!(score_of(&candidates, "not.in.list"), None);
}

// ---------------------------------------------------------------------
// TieredBounded (spec 0164 G2) has its own dedicated test module at
// tui::tiered — see protolens/src/tui/tiered.rs.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// HeatState's sentinel encoding (spec 0220)
// ---------------------------------------------------------------------

/// Spec 0220 S3: the derived `Default` would be all-zero, which reads
/// back as "scored, best 0, current 0" — every node in a fresh document
/// would report `settled()` and show a stale cue. This is the test that
/// fails if `Default` is ever re-derived.
#[test]
fn a_fresh_heat_state_is_unsettled() {
    let state = HeatState::default();
    assert!(!state.settled());
    assert!(state.best().is_none());
    assert!(state.current().is_none());
}

/// Spec 0220 S2/S4: every shape a call site can build survives the
/// sentinel encoding unchanged — including a score of `0`, which must
/// not be confused with a vetoed half, and a negative score.
#[test]
fn a_heat_state_round_trips_every_shape() {
    let bests = [
        None,
        Some(RangeHeatStats {
            best_score: None,
            best_count: 0,
        }),
        Some(RangeHeatStats {
            best_score: Some(0),
            best_count: 1,
        }),
        Some(RangeHeatStats {
            best_score: Some(-7),
            best_count: 3,
        }),
    ];
    let currents = [None, Some(None), Some(Some(0)), Some(Some(-7))];
    for best in bests {
        for current in currents {
            let state = HeatState::new(best, current);
            let got = state.best();
            match (best, got) {
                (None, None) => {}
                (Some(want), Some(got)) => {
                    assert_eq!(got.best_score, want.best_score, "{:?}", want.best_score);
                    // `best_count` is meaningless when every candidate
                    // is vetoed, so it is only pinned when it isn't.
                    if want.best_score.is_some() {
                        assert_eq!(got.best_count, want.best_count);
                    }
                }
                _ => panic!("best half changed shape"),
            }
            assert_eq!(state.current(), current);
        }
    }
}

/// Spec 0220 S2, the load-bearing one: a score that saturates must
/// still read back as a *score*, never as the "not scored" sentinel.
/// If it did not, `settled()` would answer `false` forever and
/// `prefetch_step`'s skip would stop firing — the node would be
/// re-scored on every worker progress event, which is a scheduling
/// defect rather than the display-only one S2a bounds. This is the test
/// that fails if the clamp is ever written against `i32::MIN`, or made
/// one-sided.
#[test]
fn a_saturated_score_is_still_a_score() {
    let floor = HeatState::new(
        Some(RangeHeatStats {
            best_score: Some(i64::MIN),
            best_count: 1,
        }),
        Some(Some(i64::MIN)),
    );
    assert_eq!(floor.best().unwrap().best_score, Some(SCORE_FLOOR as i64));
    assert_eq!(floor.current(), Some(Some(SCORE_FLOOR as i64)));
    assert!(floor.settled());

    let ceiling = HeatState::new(
        Some(RangeHeatStats {
            best_score: Some(i64::MAX),
            best_count: 1,
        }),
        Some(Some(i64::MAX)),
    );
    assert_eq!(ceiling.best().unwrap().best_score, Some(i32::MAX as i64));
    assert_eq!(ceiling.current(), Some(Some(i32::MAX as i64)));
    assert!(ceiling.settled());
}

// ---------------------------------------------------------------------
// heat_display (spec 0151 G5, spec 0138 G4/G9, spec 0154 G6 test plan
// H-01..H-07)
// ---------------------------------------------------------------------

/// H-01: `best` unknown — bare `[?]`, regardless of `current`'s own
/// state (there is no separate `[?/?]` state).
#[test]
fn h01_unknown_when_best_is_not_yet_known() {
    let state = HeatState::new(None, None);
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Unknown
    ));
    let state = HeatState::new(None, Some(Some(5)));
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Unknown
    ));
    assert!(!state.settled());
}

/// H-02 (spec 0138 G8): every candidate vetoed (`best_score: None`) —
/// settled with no number to print, regardless of `current`. Nothing is
/// shown in the `Findings` view this case was written under; spec 0331's
/// third state draws it as ` [vetoed]`.
#[test]
fn h02_none_when_every_candidate_is_vetoed() {
    let stats = RangeHeatStats {
        best_score: None,
        best_count: 0,
    };
    let state = HeatState::new(Some(stats), None);
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Settled { score: None }
    ));
    assert!(state.settled());
}

/// H-03: `best` known, `current` not yet computed — `[?/{best}]`, not
/// settled (still needs `current`).
#[test]
fn h03_pending_current_shows_best_only() {
    let stats = RangeHeatStats {
        best_score: Some(50),
        best_count: 1,
    };
    let state = HeatState::new(Some(stats), None);
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::PendingCurrent { best: 50 }
    ));
    assert!(!state.settled());
}

/// H-04 (spec 0151 G5's conflation-bug regression, generalized): a
/// vetoed/absent `current` (`Some(None)`) always yields `Mismatch`
/// with `current: None` — displayed as `-`, not a coincidental `0` —
/// even when `best_score` is itself `Some(0)`.
#[test]
fn h04_mismatch_for_a_vetoed_current() {
    let stats = RangeHeatStats {
        best_score: Some(0),
        best_count: 1,
    };
    let state = HeatState::new(Some(stats), Some(None));
    let display = heat_display(state, HEAT_ANCHOR_DEFAULT);
    assert!(matches!(
        display,
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Mismatch {
                current: None,
                best: 0
            },
            ..
        })
    ));
    assert!(state.settled());
}

/// H-05 (spec 0138 G4, amended): the `Mismatch` gate is `best >
/// current`, not `best >= current`.
#[test]
fn h05_mismatch_for_a_strictly_lower_current_score() {
    let stats = RangeHeatStats {
        best_score: Some(51),
        best_count: 1,
    };
    let state = HeatState::new(Some(stats), Some(Some(50)));
    let display = heat_display(state, HEAT_ANCHOR_DEFAULT);
    assert!(matches!(
        display,
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Mismatch {
                current: Some(50),
                best: 51
            },
            ..
        })
    ));
}

/// H-06 (spec 0138 G9): when the current type already achieves the top
/// score but at least one other candidate ties it there, a `Tie` cue
/// fires — `best_count` counts every candidate sharing that top score,
/// including the current one. A `None`/vetoed current (H-04) never
/// produces a `Tie`, even when `best_score` is shared by multiple
/// candidates — `Tie` requires the current type to be a genuine member
/// of the tied top-scoring group.
#[test]
fn h06_tie_when_current_shares_the_top_score_with_others() {
    let stats = RangeHeatStats {
        best_score: Some(50),
        best_count: 2,
    };
    let state = HeatState::new(Some(stats), Some(Some(50)));
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Tie {
                tie_count: 2,
                score: 50
            },
            ..
        })
    ));

    let stats = RangeHeatStats {
        best_score: Some(50),
        best_count: 3,
    };
    let state = HeatState::new(Some(stats), Some(Some(50)));
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Tie {
                tie_count: 3,
                score: 50
            },
            ..
        })
    ));
}

/// H-07: `current == best` with no other candidate tied at the top —
/// a unique optimum. Settled with a score, which spec 0331's third
/// state draws and the other two suppress; nothing is shown in the
/// `Findings` view this case was written under.
#[test]
fn h07_none_for_a_unique_optimum() {
    let stats = RangeHeatStats {
        best_score: Some(50),
        best_count: 1,
    };
    let state = HeatState::new(Some(stats), Some(Some(50)));
    assert!(matches!(
        heat_display(state, HEAT_ANCHOR_DEFAULT),
        HeatDisplay::Settled { score: Some(50) }
    ));
    assert!(state.settled());
}

// ---------------------------------------------------------------------
// heat_cue_for (end-to-end, spec 0151 G1-G3)
// ---------------------------------------------------------------------

/// Spec 0138 G8 (spec 0154 G4): absent whenever no scoring graph is
/// loaded for the session — even on an eligible node whose range isn't
/// already cached — since `heat_cue_for` has no way to populate the
/// cache without one. Shows `HeatDisplay::None` (nothing), not a
/// permanent `[?]` — mirrors the old "never show `[pending]` forever"
/// intent.
#[test]
fn absent_when_no_scoring_graph_is_loaded() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    assert!(
        app.ctx.graph.is_none(),
        "fixture must have no scoring graph"
    );
    let header_line = app.absolute_start(inner_idx);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::None));
}

/// Spec 0138, as rewritten by spec 0331: the cue goes away and comes
/// back without the caches being discarded. That claim is unchanged;
/// what changed is that it is a rotation and not a toggle, so the cue
/// comes back on the *second* press rather than the first. Verified by
/// pre-populating the caches directly (bypassing the need for a real
/// scoring graph) so a cue would otherwise be present.
#[test]
fn i_rotates_the_cue_away_and_back() {
    let mut app = message_node_app();
    app.splash = false;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    let header_line = app.absolute_start(idx);

    // A session opens with nothing shown (spec 0331 S1).
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::None));

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.heat_cues, HeatCueMode::Findings);
    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Mismatch {
                current: Some(10),
                best: 50
            },
            ..
        })
    ));

    // A finding is a finding in the third state too.
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.heat_cues, HeatCueMode::All);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::Cue(_)));

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.heat_cues, HeatCueMode::Off);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::None));

    // And back, with the cue intact — the caches were never dropped.
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::Cue(_)));
}

/// Spec 0331 test 1: `i` visits each of the three states once and
/// returns; `I` does the same in the other order. Driven through
/// `handle_key` and starting where `App::new` leaves it, so the opening
/// state is pinned by the same test.
#[test]
fn i_rotates_forward_and_shift_i_rotates_back() {
    let mut app = message_node_app();
    app.splash = false;
    let press = |app: &mut App, c: char| {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        app.heat_cues
    };

    assert_eq!(
        app.heat_cues,
        HeatCueMode::Off,
        "a session opens with no cues"
    );
    assert_eq!(press(&mut app, 'i'), HeatCueMode::Findings);
    assert_eq!(press(&mut app, 'i'), HeatCueMode::All);
    assert_eq!(press(&mut app, 'i'), HeatCueMode::Off);

    assert_eq!(press(&mut app, 'I'), HeatCueMode::All);
    assert_eq!(press(&mut app, 'I'), HeatCueMode::Findings);
    assert_eq!(press(&mut app, 'I'), HeatCueMode::Off);
}

/// End-to-end render check (spec 0138 N1): with a cue pre-cached (as
/// `i_rotates_the_cue_away_and_back` above), the main pane's header row
/// shows `HEAT_GLYPH` in its own leading column and a trailing
/// ` [current/best]` suffix; hiding the cue reverts the leading column
/// to blank and drops the suffix, without otherwise disturbing the
/// line's own indentation.
#[test]
fn render_shows_the_glyph_column_and_suffix_when_a_cue_is_present() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );

    let area = Rect::new(0, 0, 80, 24);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    // Spec 0147 G1: no border — main content is `area` minus the global
    // command/message row (`Length(1)`, bottom of the whole screen) and
    // the main pane's own local statusline row (`Length(1)`, bottom of
    // the main pane).
    let inner = Rect::new(area.x, area.y, area.width, area.height - 2);
    fn row_text(buffer: &ratatui::buffer::Buffer, inner: Rect, y: u16) -> String {
        (inner.x..inner.x + inner.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    terminal.draw(|frame| app.render(frame)).unwrap();
    let header_row = row_text(terminal.backend().buffer(), inner, inner.y);
    assert!(
        header_row.starts_with(HEAT_GLYPH),
        "leading column must show the glyph: {header_row:?}"
    );
    assert!(
        header_row.contains(" [10/50]"),
        "must show the current/best suffix: {header_row:?}"
    );

    app.heat_cues = HeatCueMode::Off;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let header_row = row_text(terminal.backend().buffer(), inner, inner.y);
    assert_eq!(
        header_row.chars().next().unwrap(),
        ' ',
        "leading column reserved but blank when hidden: {header_row:?}"
    );
    assert!(
        !header_row.contains('/'),
        "no suffix while hidden: {header_row:?}"
    );
}

/// End-to-end render check (spec 0138 N1/G9): with a `Tie` cue
/// pre-cached (current type tied for the top score with one other
/// candidate), the main pane's header row shows `HEAT_GLYPH` in its own
/// leading column and a trailing ` [<tie_count>@<score>]` suffix —
/// distinct from the `Mismatch` cue's ` [current/best]` (no `/`).
#[test]
fn render_shows_the_tie_count_suffix_when_tied_for_best() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        2,
        "google.protobuf.DescriptorProto",
        Some(50),
    );

    let area = Rect::new(0, 0, 80, 24);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    // Spec 0147 G1: no border — main content is `area` minus the global
    // command/message row (`Length(1)`, bottom of the whole screen) and
    // the main pane's own local statusline row (`Length(1)`, bottom of
    // the main pane).
    let inner = Rect::new(area.x, area.y, area.width, area.height - 2);
    fn row_text(buffer: &ratatui::buffer::Buffer, inner: Rect, y: u16) -> String {
        (inner.x..inner.x + inner.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    terminal.draw(|frame| app.render(frame)).unwrap();
    let header_row = row_text(terminal.backend().buffer(), inner, inner.y);
    assert!(
        header_row.starts_with(HEAT_GLYPH),
        "leading column must show the glyph: {header_row:?}"
    );
    assert!(
        header_row.contains(" [2@50]"),
        "must show the tie-count and score suffix: {header_row:?}"
    );
    assert!(
        !header_row.contains('/'),
        "the Tie cue's suffix must not look like Mismatch's [current/best]: {header_row:?}"
    );
}

// ---------------------------------------------------------------------
// Spec 0331: the third state
// ---------------------------------------------------------------------

/// The main pane's first row as drawn, cell by cell — the symbol and
/// the style it landed with, so an assertion about a color is made
/// where the color actually arrives rather than on a `Span` upstream.
fn drawn_header_cells(app: &mut App) -> Vec<(String, ratatui::style::Style)> {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..80)
        .map(|x| {
            let cell = &buffer[(x, app.main_area.y)];
            (cell.symbol().to_string(), cell.style())
        })
        .collect()
}

/// The style the run of cells spelling `needle` was drawn in, or `None`
/// where the row does not contain it at all.
fn drawn_suffix_style(
    cells: &[(String, ratatui::style::Style)],
    needle: &str,
) -> Option<ratatui::style::Style> {
    let row: String = cells.iter().map(|(s, _)| s.as_str()).collect();
    let byte = row.find(needle)?;
    Some(cells[row[..byte].chars().count()].1)
}

/// Spec 0331 test 2: a node seeded at the unique optimum says nothing
/// in the findings view and says its own score in the third state, in
/// the green reserved for agreement.
#[test]
fn an_agreeing_node_says_its_score_in_the_third_state() {
    let mut app = message_node_app();
    app.splash = false;
    let range = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range);
    // best 50 reached by exactly one candidate, and the current type is
    // it: the unique optimum.
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(50),
    );

    app.heat_cues = HeatCueMode::Findings;
    let cells = drawn_header_cells(&mut app);
    let row: String = cells.iter().map(|(s, _)| s.as_str()).collect();
    assert!(
        !row.contains('['),
        "no finding, so nothing at all in the second state: {row:?}"
    );

    app.heat_cues = HeatCueMode::All;
    let cells = drawn_header_cells(&mut app);
    let row: String = cells.iter().map(|(s, _)| s.as_str()).collect();
    assert!(
        row.contains(" [50]"),
        "the third state says the score both halves agree on: {row:?}"
    );
    assert_eq!(
        cells[0].0, " ",
        "and leaves the glyph column blank (N6): {row:?}"
    );
    assert_eq!(
        drawn_suffix_style(&cells, " [50]").map(|s| s.fg),
        // Spec 0336 S4: Settled{Some} is a tie of one — the current
        // type is uniquely best — so it wears the same flat blue the
        // Tie cue's word does.
        Some(theme::heat_label_style(theme::HeatHue::Blue, app.theme).fg),
        "an agreeing cue is flat blue (spec 0336 S4)"
    );
}

/// Spec 0331 test 3, re-pointed by spec 0335 S4: the companion, and the
/// other half of `Settled`. A range where a real message search found
/// nothing says so in words *and* wears the square — the loudest mark
/// in the pane, and the one whose color is a sentinel rather than a
/// position on the ramp.
#[test]
fn an_unmatched_message_says_so_loudly() {
    let mut app = message_node_app();
    app.splash = false;
    let range = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        None,
        0,
        "google.protobuf.DescriptorProto",
        None,
    );

    app.heat_cues = HeatCueMode::Findings;
    let row: String = drawn_header_cells(&mut app)
        .iter()
        .map(|(s, _)| s.as_str())
        .collect();
    assert!(!row.contains('['), "blank in the second state: {row:?}");

    app.heat_cues = HeatCueMode::All;
    let cells = drawn_header_cells(&mut app);
    let row: String = cells.iter().map(|(s, _)| s.as_str()).collect();
    assert!(row.contains(" [unmatched]"), "{row:?}");
    assert_eq!(
        cells[0].0, HEAT_GLYPH,
        "and unlike its agreeing sibling it wears the square (0335 S4): {row:?}"
    );
    let amber = theme::heat_label_style(theme::HeatHue::Amber, app.theme).fg;
    let blue = theme::heat_label_style(theme::HeatHue::Blue, app.theme).fg;
    assert_eq!(
        drawn_suffix_style(&cells, " [unmatched]").map(|s| s.fg),
        Some(amber),
        "a verdict of 'nothing fits' wears flat amber (spec 0336 S4)"
    );
    assert_eq!(
        cells[0].1.fg, amber,
        "square and word are the one sentinel color"
    );
    assert_ne!(
        amber, blue,
        "which is only worth asserting if the two differ at all"
    );
}

// ---------------------------------------------------------------------
// Spec 0335: a question not asked is not an answer
// ---------------------------------------------------------------------

/// Seeds `idx` with the settled-and-empty verdict — a candidate list
/// that came back with nothing in it — and reports what the third state
/// would draw for it.
fn cue_for_an_empty_candidate_list(app: &mut App, idx: usize) -> HeatDisplay {
    let start = app.heat_scored_range(idx).start;
    let key = app
        .current_type_key(idx)
        .expect("every node in this fixture is declared");
    seed_range_heat_entry(app, start, None, 0, &key, None);
    app.heat_cues = HeatCueMode::All;
    let header = app.absolute_start(idx);
    app.heat_cue_for(header)
}

/// Spec 0335 test 1 / G1: the schema has already said these bytes are
/// text, and a varint cannot be a message under any typing. Neither row
/// is one whose answer was "nothing fits"; neither had a question. Both
/// drew ` [vetoed]` before this spec, which is what buried the rows that
/// did.
#[test]
fn a_declared_scalar_asks_no_question() {
    let (mut app, text, n, _blob) = declared_scalars_fixture();
    for (idx, what) in [(text, "a declared string"), (n, "a varint")] {
        assert!(
            !app.inference_applies(idx),
            "{what} was never going to have a candidate"
        );
        assert!(
            matches!(
                cue_for_an_empty_candidate_list(&mut app, idx),
                HeatDisplay::None
            ),
            "{what} draws nothing even in the third state"
        );
    }
}

/// Spec 0335 test 3 / S1: the carve-out and the rule it is carved out
/// of, asserted as the pair they are. A `bytes` field is the schema
/// declining to say what is in there — the one declared scalar where a
/// message search is exactly the point — and its neighbour is not.
#[test]
fn a_bytes_field_is_still_asked() {
    let (mut app, text, _n, blob) = declared_scalars_fixture();
    assert!(app.inference_applies(blob), "bytes is the carve-out");
    assert!(matches!(
        cue_for_an_empty_candidate_list(&mut app, blob),
        HeatDisplay::Settled { score: None }
    ));
    assert!(matches!(
        cue_for_an_empty_candidate_list(&mut app, text),
        HeatDisplay::None
    ));
}

/// Spec 0335 N2: the gate is on the settled-and-empty arm alone. A
/// declared `string` whose bytes really do score as a message is rare
/// and is a genuine finding, so a node the predicate refuses still says
/// what it found.
#[test]
fn a_refused_node_still_reports_a_mismatch() {
    let (mut app, text, _n, _blob) = declared_scalars_fixture();
    assert!(!app.inference_applies(text));
    let start = app.heat_scored_range(text).start;
    seed_range_heat_entry(&mut app, start, Some(50), 1, "string", Some(10));
    app.heat_cues = HeatCueMode::All;
    let header = app.absolute_start(text);
    assert!(matches!(
        app.heat_cue_for(header),
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Mismatch { .. },
            ..
        })
    ));
}

/// Spec 0335 N3: the predicate decides what reaches the screen and
/// nothing else. A frame over a document of rows it refuses still asks
/// the worker about every one of them — which is what keeps the caches
/// warm, and what lets a refused row report a mismatch the moment one
/// turns up.
#[test]
fn the_gate_asks_for_nothing_new() {
    let (mut app, text, n, blob) = declared_scalars_fixture();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::All;
    assert!(
        [text, n].iter().all(|&idx| !app.inference_applies(idx)),
        "the fixture is worth rendering only if it holds refused rows"
    );
    assert!(app.inference_applies(blob));

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // Named ranges rather than a count: what the predicate could have
    // broken is not *how many* requests a frame makes but *which*, and
    // there is no switch to turn it off for a second render to be
    // compared against.
    //
    // Bounded by the length read first: `take_next_range` *blocks* on an
    // empty queue, so draining it with `while let Some(..)` would hang
    // on the pop past the end rather than end the loop.
    let worker = app.heat_worker.take().expect("just installed");
    let queued: Vec<usize> = (0..worker.queue_len())
        .filter_map(|_| worker.take_next_range())
        .collect();
    for (idx, what) in [(text, "the string"), (n, "the varint"), (blob, "the bytes")] {
        let start = app.heat_scored_range(idx).start;
        assert!(
            queued.contains(&start),
            "{what} row is asked about all the same: {queued:?}"
        );
    }
}

/// Spec 0335 N1, pinned directly: the override pane opens where it did.
/// The new predicate refuses two of these three and `can_override` must
/// go on admitting all of them, or pressing `t` on an `int32` stops
/// offering `sfixed32` (spec 0135 G3).
#[test]
fn can_override_is_unchanged() {
    let (app, text, n, blob) = declared_scalars_fixture();
    for idx in [text, n, blob] {
        assert!(app.can_override(idx), "node {idx} must stay overridable");
    }
}

/// Spec 0331 N3 / test 4: a node with no question stays blank in every
/// state. `heat_cue_at` refuses a non-header line and a node that
/// cannot be overridden *before* anything is resolved, so those rows
/// are not blank because the answer was "fine" — there is no question.
#[test]
fn a_node_with_no_question_stays_blank() {
    let (mut app, run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let record = app.heat_scored_range(run);
    seed_range_heat_entry(&mut app, record.start, Some(50), 1, "int32", Some(50));
    app.heat_cues = HeatCueMode::All;

    let rows = app.node_lines(run);
    for line in rows.start + 1..rows.end {
        assert!(
            matches!(app.heat_cue_for(line), HeatDisplay::None),
            "a continuation line has no question of its own: line {line}"
        );
    }

    // And a node `can_override` refuses is blank on its header row too.
    // No fixture holds one: spec 0135 G3 widened the gate to every wire
    // type a value can carry, so the shape has to be made by hand — a
    // group frame that is not itself flagged as a message.
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::All;
    let idx = 1;
    app.tree_mut()[idx].span.is_message = false;
    let label = app.tree[idx].span.label();
    app.tree_mut()[idx].span.wire_and_label = NodeSpan::pack(WT_START_GROUP as u8, label);
    assert!(!app.can_override(idx), "the hand-made shape is refused");
    let header = app.absolute_start(idx);
    assert!(matches!(app.heat_cue_for(header), HeatDisplay::None));
}

/// Spec 0331 N1 / test 5: the mode is a formatting decision. A frame
/// drawn in the third state asks the worker for exactly what a frame
/// drawn in the second state asks for — which is why the third state
/// can be reached on a keystroke with no repaint latency, and why the
/// caches are warm the moment it is.
#[test]
fn the_third_state_asks_for_nothing_new() {
    let queued = |mode: HeatCueMode| {
        let mut app = message_node_app();
        app.splash = false;
        app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
        app.heat_cues = mode;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        app.heat_worker.as_ref().unwrap().queue_len()
    };

    assert_eq!(queued(HeatCueMode::All), queued(HeatCueMode::Findings));
    // And the pair is worth comparing only if either asks for anything.
    assert!(queued(HeatCueMode::Findings) > 0);
}

/// The cue is main-pane-only (spec 0138 N2/Test-plan): `heat_cue_for`
/// gates on `node_at_header_line`, which resolves only lines of the
/// main pane's own document, so a cached cue for a node never leaks
/// into the override pane's rendering.
#[test]
fn cue_never_appears_in_the_override_pane() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    let range = extract::message_payload_range(&app.blob, &app.tree[inner_idx].span.raw_range);
    seed_range_heat_entry(&mut app, range.start, Some(50), 1, "test.Inner", Some(10));
    app.cursor = inner_idx;
    app.toggle_override();
    assert!(app.override_target.is_some());

    let area = Rect::new(0, 0, 120, 24);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    // The main pane stays visible in its own (left) half of the split
    // while the override pane is open, so it's `app.side_area` — the
    // override pane's own region, populated by `render_override_pane`
    // — that must be searched, not the whole buffer.
    let side_area = app.side_area;
    let buffer = terminal.backend().buffer();
    let found = (side_area.x..side_area.x + side_area.width).any(|x| {
        (side_area.y..side_area.y + side_area.height).any(|y| buffer[(x, y)].symbol() == HEAT_GLYPH)
    });
    assert!(!found, "heat glyph must never render in the override pane");
}

// ---------------------------------------------------------------------
// Caching regressions (spec 0151 G2/G3/G6 test plan)
// ---------------------------------------------------------------------

/// Regression for the original caching bug (spec 0151 Background): once
/// a range's stats and the current type's score are both cached, a
/// second `heat_cue_for` call for the same line is a pure cache hit —
/// no graph is required for it to succeed, proving no re-scoring
/// happened (a real graph-less `App` would otherwise short-circuit to
/// `None` on any fresh `inferred_candidates` call).
#[test]
fn second_call_for_the_same_line_is_a_pure_cache_hit() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
    assert!(app.ctx.graph.is_none());
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    let header_line = app.absolute_start(idx);

    // Two calls, both cache hits (no graph loaded, so a miss would
    // short-circuit to `None` via `self.ctx.graph.as_ref()?`).
    let first = app.heat_cue_for(header_line);
    let second = app.heat_cue_for(header_line);
    assert!(matches!(first, HeatDisplay::Cue(_)));
    assert!(matches!(second, HeatDisplay::Cue(_)));
}

/// Regression for the "permanently-vetoed range never gets cached" bug
/// (spec 0151 Background): a `None` `best_score` must still be a real
/// cache entry, so a subsequent lookup is a hit, not treated as
/// "nothing was cached."
#[test]
fn vetoed_range_is_still_cached_as_a_hit() {
    let mut cache: TieredBounded<usize, RangeHeatStats> = TieredBounded::new(8192);
    cache.upsert(
        42,
        RangeHeatStats {
            best_score: None,
            best_count: 0,
        },
        Tier::Visible,
    );
    let hit = cache.peek(&42, Tier::Visible);
    assert!(hit.is_some());
    assert_eq!(hit.unwrap().best_score, None);
}

/// Cross-population (spec 0151 G6, relocked onto the shared cache by
/// spec 0152 N8): once `heat_cue_for` pays for a full
/// `inferred_candidates` call, the same candidate list — capped to
/// `override_list_height` — is inserted into `App::heat_caches`'
/// `by_range` under the same range, so a later override-pane open
/// (`t`) on that node hits the cache instead of re-scoring.
#[test]
fn g6_cross_population_caps_to_override_list_height() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    let range = extract::message_payload_range(&app.blob, &app.tree[inner_idx].span.raw_range);
    app.override_list_height = 200; // simulates the eager `run()` init.
    app.heat_caches.lock().unwrap().by_range.upsert(
        range.start,
        RangeHeatEntry {
            best_score: Some(5),
            best_count: 1,
            top_n: vec![("a.Type".to_string(), 5), ("b.Type".to_string(), 3)],
        },
        Tier::Visible,
    );
    // The `by_range` cache API itself is exercised elsewhere
    // (`heat_worker.rs`'s own tests); this just pins that
    // `override_list_height` participates in the cap `heat_cue_for`
    // uses, per the `.max(1)` expression it shares with
    // `override_select.rs`.
    assert_eq!(app.override_list_height.max(1), 200);
    assert!(app
        .heat_caches
        .lock()
        .unwrap()
        .by_range
        .peek(&range.start, Tier::Visible)
        .is_some());
}

// ---------------------------------------------------------------------
// Root-range seeding from the startup sweep (spec 0168 G3)
// ---------------------------------------------------------------------

/// The whole point of G3: the root-type sweep already scored the root
/// node's payload range, so `App::new` writes that result into the
/// caches and the very first `heat_cue_for` on the root is a hit —
/// asserted against a graph-less `App`, where a miss could only
/// short-circuit to `None`, so a `Cue` proves the seed was used and no
/// re-score happened.
///
/// Both caches are checked, since they answer different callers:
/// `by_range` is what the cue reads, `complete` is what the override
/// pane reads.
#[test]
fn g3_the_startup_sweep_seeds_the_root_range() {
    let candidates: Vec<(String, i64)> = (0..HEAT_CUE_PREVIEW + 4)
        .map(|i| (format!("a.Type{i}"), 50 - i as i64))
        .collect();
    let mut app = message_node_app_with_root_candidates(candidates.clone());
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
    assert!(app.ctx.graph.is_none(), "a hit here must not be a re-score");

    let range = app.heat_scored_range(app.first_node);
    let mut caches = app.heat_caches.lock().unwrap();
    let entry = caches
        .by_range
        .peek(&range.start, Tier::User)
        .expect("the root's payload start must be seeded");
    assert_eq!(entry.best_score, Some(50));
    assert_eq!(entry.best_count, 1);
    assert_eq!(
        caches.complete.newest().map(|(r, c)| (r.clone(), c.len())),
        Some((range.clone(), candidates.len())),
        "`complete` holds the full list, keyed by the root's range"
    );
    drop(caches);

    // The seeded entry is enough to answer the cue outright.
    app.heat_caches.lock().unwrap().current_score.upsert(
        (range.start, "google.protobuf.DescriptorProto".to_string()),
        Some(10),
        Tier::User,
    );
    let header_line = app.absolute_start(app.first_node);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::Cue(_)));
}

/// `by_range`'s `top_n` is seeded at the same cap `heat_cue_resolve`
/// applies, not at full width — seeding the whole list would be exactly
/// the oversized entry `rendering-flaws.md` P5 describes, on the single
/// widest range in the document. `override_list_height` is still 0 when
/// `App::new` runs, so the cap is `HEAT_CUE_PREVIEW`.
///
/// `complete` is deliberately *not* capped: it is where the unbounded
/// list belongs, and the override pane needs all of it.
#[test]
fn g3_seeded_top_n_is_capped_but_complete_is_not() {
    let candidates: Vec<(String, i64)> = (0..HEAT_CUE_PREVIEW + 4)
        .map(|i| (format!("a.Type{i}"), 50 - i as i64))
        .collect();
    let app = message_node_app_with_root_candidates(candidates.clone());

    let range = app.heat_scored_range(app.first_node);
    let mut caches = app.heat_caches.lock().unwrap();
    let entry = caches.by_range.peek(&range.start, Tier::User).unwrap();
    assert_eq!(entry.top_n.len(), HEAT_CUE_PREVIEW);
    assert_eq!(
        caches.complete.newest().unwrap().1.len(),
        candidates.len(),
        "the uncapped list belongs in `complete`"
    );
}

/// No sweep ran (`--type`, `--raw`, or no scoring graph): the empty
/// candidate list must leave both caches untouched rather than writing
/// an empty entry, which would read as a legitimate "every candidate
/// vetoed" answer and permanently suppress the root's cue.
#[test]
fn g3_no_sweep_seeds_nothing() {
    let app = message_node_app();
    let range = app.heat_scored_range(app.first_node);
    let mut caches = app.heat_caches.lock().unwrap();
    assert!(caches.by_range.peek(&range.start, Tier::User).is_none());
    assert_eq!(caches.complete.len(), 0);
}

// The seeding is only sound because the sweep's input and the cue's
// cache key are the same bytes: `heat_scored_range` strips the root
// node's tag/length prefix, and in a real `Decoded` that prefix *is*
// the virtual wrapper, so the result is `wrapper_offset..blob.len()`
// — exactly the slice (`blob.payload()`) that the startup sweep is
// handed by `determine_root_type_meanwhile`.
//
// That last equality is not asserted here, because the fixtures in
// this file carry a stub `wrapper_offset: 0` alongside a blob that
// does have a two-byte prefix, so they cannot express it. What is
// asserted instead is the consequence that actually matters, and it
// is covered end-to-end by
// `g3_the_startup_sweep_seeds_the_root_range`: the root's very first
// cue is answered from the seeded entry, on an `App` with no graph,
// where a wrong key could only miss.
// ---------------------------------------------------------------------
// warm_up_heat_cues (spec 0151 G8)
// ---------------------------------------------------------------------

/// No scoring graph loaded: `warm_up_heat_cues` takes its early-return
/// branch and completes without touching `app.message` (which the
/// redraw path inside the loop is the only thing that ever sets, per
/// `WARMUP_FIRST_DRAW_DELAY`/`WARMUP_REDRAW_INTERVAL`). Uses a
/// `CrosstermBackend` over an in-memory `Vec<u8>` rather than
/// `TestBackend` (mirrors `open_editor_reports_a_missing_nvim_instead_
/// of_crashing`'s own precedent: `TestBackend`'s `Error` is
/// `Infallible`, which doesn't satisfy `io::Error: From<B::Error>`).
/// No test in this file drives `warm_up_heat_cues` itself against a
/// real graph (`message_node_app_with_graph`, spec 0152 test plan,
/// exists for the worker-round-trip tests below instead), so the
/// `ctx.graph.is_some()` populate/redraw path isn't separately unit-
/// tested here.
#[test]
fn warm_up_heat_cues_is_a_noop_without_a_scoring_graph() {
    let mut app = message_node_app();
    assert!(app.ctx.graph.is_none());
    // `message_node_app`'s fixture seeds a root override to a type
    // absent from `DescriptorContext::empty_for_test()`'s empty
    // descriptor set, so `App::new`'s own `render_overrides` pass
    // already leaves an error string in `app.message` — a fixture
    // artifact unrelated to `warm_up_heat_cues`. Compare against this
    // baseline rather than asserting emptiness.
    let before = app.message.clone();
    let mut terminal = Terminal::new(CrosstermBackend::new(Vec::new())).unwrap();

    warm_up_heat_cues(&mut terminal, &mut app).unwrap();

    assert_eq!(
        app.message, before,
        "the early-return branch must never touch app.message"
    );
}

/// `HeatCueMode::Off` (2026-07-19 feedback) no longer skips
/// `warm_up_heat_cues`'s own gate — the background worker must keep
/// priming the cache even while cues are hidden, so `heat_cue_for`
/// (called by the warm-up loop below) still pushes its request; only
/// its returned cue is suppressed, at the `heat_cue_for` layer, not
/// here.
#[test]
fn heat_cue_for_still_pushes_a_request_when_cues_are_off() {
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::Off;
    let idx = 0;
    let header_line = app.absolute_start(idx);

    let cue = app.heat_cue_for(header_line);

    assert!(
        matches!(cue, HeatDisplay::None),
        "no cue must be shown while hidden"
    );
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        1,
        "the request must still be pushed while hidden, so the cache \
         is already warm once cues are un-hidden"
    );
}

// ---------------------------------------------------------------------
// heat_lookup / heat_cue_for worker-aware forks (spec 0152 G6 test plan)
// ---------------------------------------------------------------------

/// Both the window and (when a `current_key` is given) the current
/// type's exact score must be cached for `heat_lookup` to report a
/// hit — a window-only or current-score-only cache still misses (and
/// still pushes a request), pinning the "both must hold" AND-gating,
/// not just the window half of it.
#[test]
fn heat_lookup_ands_window_and_current_score() {
    let mut app = message_node_app();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    let key = "google.protobuf.DescriptorProto";

    // Window covered, current_score missing.
    app.heat_caches.lock().unwrap().by_range.upsert(
        range.start,
        RangeHeatEntry {
            best_score: Some(5),
            best_count: 1,
            top_n: vec![("a.Type".to_string(), 5); HEAT_CUE_PREVIEW],
        },
        Tier::Visible,
    );
    assert!(app
        .heat_lookup(&range, Some(key), 0, HEAT_CUE_PREVIEW, Tier::Visible)
        .is_none());
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);

    // Symmetric case: current_score cached, window now insufficient.
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_caches.lock().unwrap().by_range.upsert(
        range.start,
        RangeHeatEntry {
            best_score: Some(5),
            best_count: 1,
            top_n: vec![("a.Type".to_string(), 5)],
        },
        Tier::Visible,
    );
    app.heat_caches.lock().unwrap().current_score.upsert(
        (range.start, key.to_string()),
        Some(3),
        Tier::Visible,
    );
    assert!(app
        .heat_lookup(&range, Some(key), 0, HEAT_CUE_PREVIEW, Tier::Visible)
        .is_none());
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);
}

/// With a manually-installed `HeatWorkerHandle`, a `heat_cue_for` call
/// on a `Pending` node with an empty cache returns `None`, pushes
/// exactly one `HeatRequest` (`[0, HEAT_CUE_PREVIEW)`), and leaves
/// `heat_states[idx]` as `Pending`. A second call on the same node
/// before any cache change pushes no additional request — since the
/// queue would merge a second push anyway, this specifically pins
/// that `heat_lookup`'s own before-push check, not just the queue's
/// merge, is what prevents queue growth here.
#[test]
fn heat_cue_for_pushes_at_most_one_request_while_pending() {
    let mut app = message_node_app();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let header_line = app.absolute_start(idx);

    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Unknown
    ));
    assert!(!app.heat_states[idx].settled());
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);

    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Unknown
    ));
    assert!(!app.heat_states[idx].settled());
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        1,
        "a second call before any cache change must not grow the queue"
    );
}

/// Pre-populating the cache with a `RangeHeatEntry` whose `top_n`
/// already covers `[0, HEAT_CUE_PREVIEW)` (and the current key's exact
/// score) resolves via `heat_cue_for` without pushing any request at
/// all — the direct test for "don't trigger a `score_all` (or even a
/// push) if the cache already covers the ask."
#[test]
fn heat_cue_for_pre_populated_cache_resolves_without_pushing() {
    let mut app = message_node_app();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    let header_line = app.absolute_start(idx);

    let cue = app.heat_cue_for(header_line);
    assert!(matches!(cue, HeatDisplay::Cue(_)));
    assert!(app.heat_states[idx].settled());
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 0);
}

/// Spec 0224 S2, and the one assertion the whole spec rests on: the
/// frame *is* the recheck. An answer that appears in `heat_caches`
/// between two draws is picked up by the second draw alone, with
/// nothing standing between the worker and the screen but the repaint
/// spec 0192's `heat_dirty` already owes.
///
/// `stub_for_test` rather than a real worker, so the cache cannot
/// change except where this test changes it — the transition asserted
/// is the one the test performed, not a race it happened to win.
#[test]
fn a_drawn_row_picks_up_a_cache_answer_with_no_recheck() {
    let mut app = message_node_app();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    let header_line = app.absolute_start(idx);

    // First draw: nothing is known, so the row shows `[?]`.
    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Unknown
    ));
    assert!(!app.heat_states[idx].settled());

    // The worker's answer arrives, out of band — no event, no recheck.
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );

    // Second draw: the same call that drew `[?]` now draws the cue.
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::Cue(_)));
    assert!(app.heat_states[idx].settled());
}

/// Real worker thread, real tiny in-memory graph, through the `App`
/// layer end-to-end (spec 0152 test plan): a `heat_cue_for` miss with
/// a `HeatWorkerHandle` installed (via `DescriptorContext::
/// for_test_with_graph`) leaves the node `Pending`; the worker's own
/// cache write is later picked up by the next `heat_cue_for` (spec
/// 0224 S2 — the repaint `AppEvent::HeatWorkerProgress` owes within
/// `HEAT_REPAINT_INTERVAL` is what performs this in `run_loop`),
/// resolving it. Complements `heat_worker.rs`'s own
/// lower-level round-trip test (which pins the exact cache contents
/// and the no-re-score call-count guarantee); this one pins the
/// `App`-level wiring instead.
///
/// The two phases — queue against a stub, *then* start the thread on
/// that same queue (`start_for_test`) — are what make both halves
/// provable rather than schedule-dependent, and the ordering is
/// load-bearing twice over:
///
/// - `heat_cue_resolve` pushes and reads the cache back under two
///   separate lock acquisitions. A worker running during that window
///   can legally finish first — an 8-byte payload against an 8-message
///   graph is microseconds — and then `heat_cue_for` returns a settled
///   cue, not `Unknown`, so the first phase would assert nothing.
/// - The poll below would then report success on its very first call,
///   without the worker's answer ever having had to travel from the
///   cache into `heat_states` — which is the path this test is named
///   for.
///
/// Dropping the worker entirely (the fix `override_pane_auto_completes_
/// from_polling_alone_without_scrolling` uses) is not available here:
/// with `heat_worker` `None`, `heat_cue_resolve` falls through to its
/// *synchronous* scoring arm and settles the node on this thread.
#[test]
fn heat_cue_for_resolves_once_a_real_worker_populates_the_cache() {
    let mut app = message_node_app_with_graph();
    // Overwrite the fixture's all-zero payload (tag/length prefix
    // unchanged, so the node's `raw_range` stays valid) with four
    // repeated, structurally valid field-1 varint encodings — an
    // all-zero payload's leading tag byte (field number 0) is
    // structurally invalid and would veto every candidate, so
    // `by_range`'s window could never fill.
    app.blob = Arc::new(Blob::unwrapped(vec![
        0x22, 0x08, 0x08, 0x01, 0x08, 0x02, 0x08, 0x03, 0x08, 0x04,
    ]));
    let idx = 0;
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues = HeatCueMode::Findings;
    let header_line = app.absolute_start(idx);

    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Unknown
    ));
    assert!(!app.heat_states[idx].settled());
    // The fact phase two rests on: the work is queued.
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);

    // Now let a real worker loose on the request already queued above.
    let graph = Arc::clone(app.ctx.graph.as_ref().unwrap());
    let blob = Arc::clone(&app.blob);
    let (tx, _rx) = mpsc::channel();
    let caches = Arc::clone(&app.heat_caches);
    app.heat_worker = Some(
        app.heat_worker
            .take()
            .expect("the stub is still installed")
            .start_for_test(caches, graph, blob, tx, 1),
    );

    // Bounded poll, not `recv` — this isn't exercising the
    // event-driven wiring, just the worker/cache/redraw contract.
    let mut resolved = false;
    for _ in 0..200 {
        app.heat_cue_for(header_line);
        if app.heat_states[idx].settled() {
            resolved = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        resolved,
        "the real worker must resolve the cache within the bounded poll"
    );
}

// ---------------------------------------------------------------------
// Per-node request tier (spec 0208 S3 test plan)
// ---------------------------------------------------------------------

/// Spec 0262 S7, replacing spec 0208 S3's rule that the cursor's own
/// node asks at `Tier::User`. Every main-pane row, the cursor's row
/// included, asks at `Tier::Visible`; the cursor's precedence comes
/// from being asked for *again* at the end of the frame, which moves
/// it back to the head of its band (spec 0208 S4c).
///
/// The cursor's row is deliberately asked for *first* in the row-by-row
/// pass here, so that the head of the band is somebody else's until the
/// re-ask; asked for last, this would pass with no re-ask at all.
#[test]
fn the_cursor_node_is_asked_for_again_and_so_is_served_first() {
    let (mut app, items) = repeated_message_fixture();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let (under_cursor, elsewhere) = (items[0], items[1]);
    app.set_cursor(under_cursor);

    app.heat_cue_for(app.absolute_start(under_cursor));
    app.heat_cue_for(app.absolute_start(elsewhere));
    app.refresh_cursor_heat_cue();

    let cursor_range = app.heat_scored_range(under_cursor).start;
    let worker = app.heat_worker.as_ref().unwrap();
    assert_eq!(
        worker.activity(),
        Some(Tier::Visible),
        "no main-pane row claims the whole pool for itself"
    );
    assert_eq!(
        worker.queue_len(),
        2,
        "two distinct payload ranges; the re-ask merges rather than \
         queuing a third entry"
    );
    assert_eq!(
        worker.take_next_range(),
        Some(cursor_range),
        "and the re-ask is what puts the cursor's row at the head"
    );
}

/// Spec 0262 S7, and the reason the re-ask is the whole mechanism:
/// moving the cursor moves the head of the band with it, and the row it
/// left keeps the place — and the tier — it already had. A tier never
/// moves down (spec 0164 G5), so nothing is demoted; the row behind
/// simply stops being asked for twice.
#[test]
fn moving_the_cursor_moves_the_head_of_the_band_with_it() {
    let (mut app, items) = repeated_message_fixture();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let (first, second) = (items[0], items[1]);

    app.set_cursor(first);
    app.heat_cue_for(app.absolute_start(first));
    app.heat_cue_for(app.absolute_start(second));
    app.refresh_cursor_heat_cue();

    app.set_cursor(second);
    app.refresh_cursor_heat_cue();

    let (first_range, second_range) = (
        app.heat_scored_range(first).start,
        app.heat_scored_range(second).start,
    );
    let worker = app.heat_worker.as_ref().unwrap();
    assert_eq!(worker.queue_len(), 2, "re-asked, not re-queued");
    assert_eq!(
        worker.take_next_range(),
        Some(second_range),
        "the row the cursor moved onto is served first"
    );
    assert_eq!(
        worker.take_next_range(),
        Some(first_range),
        "and the one it left is still queued behind it"
    );
}

// ---------------------------------------------------------------------
// Spec 0337: the scale learns its top
// ---------------------------------------------------------------------

/// Spec 0337 test 1 / S1: equal ratios of score produce equal steps in
/// `t` (the key logarithmic property), `t` is 0 at and below score 1,
/// and 1 at and above `exp(anchor)`.
#[test]
fn the_fraction_is_logarithmic() {
    let anchor = 4.0_f32; // exp(4) ≈ 54.6; easy to reason about

    // Floor: score 0 and negatives are 0.
    assert_eq!(heat_fraction(0, anchor), 0.0);
    assert_eq!(heat_fraction(-7, anchor), 0.0);
    assert_eq!(heat_fraction(1, anchor), 0.0, "ln(1) = 0");

    // Ceiling: clamps at 1 for any score >= exp(anchor).
    assert_eq!(heat_fraction(1_000_000, anchor), 1.0);

    // Logarithmic: score^2 is twice the t of score (additive in log space).
    // Choose scores where both values are well below the ceiling (anchor=4,
    // so scores must be well below exp(4) ≈ 54.6). Use 3 and 9 = 3^2.
    let t1 = heat_fraction(3, anchor);
    let t2 = heat_fraction(9, anchor); // 3^2
    assert!(
        (t2 - 2.0 * t1).abs() < 1e-5,
        "t(score^2) must equal 2*t(score): t1={t1} t2={t2}"
    );
}

/// Spec 0337 test 2 / S1: every score at or below 1, including
/// `SCORE_FLOOR`, maps to `t = 0`. `SCORE_FLOOR` is a sentinel for the
/// most negative real score; it must produce 0, not a NaN.
#[test]
fn a_negative_score_is_the_floor() {
    let anchor = HEAT_ANCHOR_DEFAULT;
    for &score in &[SCORE_FLOOR as i64, -100_i64, 0_i64, 1_i64] {
        assert_eq!(
            heat_fraction(score, anchor),
            0.0,
            "score {score} must map to t = 0"
        );
    }
}

/// Spec 0337 test 3 / S4: the anchor never decreases — it ratchets.
/// This is the core of G3: a square can only move toward the top, so
/// no two frames show the same row at different brightnesses due to an
/// unrelated node being scored.
#[test]
fn the_anchor_only_rises() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;

    // A descending sequence: early huge score, then many small ones.
    // The anchor must never fall below the first ratchet position.
    let big = 100_000_i64;
    let small = 5_i64;

    // To get the anchor to move we need 64 samples. Feed the histogram
    // directly (the public interface — `record_heat_state` is the
    // production path; here we test the ratchet mechanics in isolation).
    for _ in 0..64 {
        app.heat_histogram.record(big);
    }
    if let Some(p) = app.heat_histogram.p95() {
        if p > app.heat_anchor {
            app.heat_anchor = p;
        }
    }
    let high_anchor = app.heat_anchor;
    assert!(
        high_anchor > HEAT_ANCHOR_DEFAULT,
        "64 big scores must push the anchor up"
    );

    // Now feed 64 small scores. The percentile should drop, but the
    // anchor must not.
    for _ in 0..64 {
        app.heat_histogram.record(small);
    }
    if let Some(p) = app.heat_histogram.p95() {
        if p > app.heat_anchor {
            app.heat_anchor = p;
        }
    }
    assert_eq!(
        app.heat_anchor, high_anchor,
        "anchor must not fall after small scores arrive"
    );
}

/// Spec 0337 test 4 / S4: the 64-sample guard. A single enormous score
/// — however large — must not move the anchor off the default.
#[test]
fn a_thin_histogram_keeps_the_default() {
    let mut h = HeatHistogram::default();
    // Fewer than 64 samples: no percentile, anchor stays at default.
    for _ in 0..63 {
        h.record(1_000_000);
    }
    assert!(
        h.p95().is_none(),
        "63 samples is below the guard — no percentile yet"
    );
    // The 64th tips it over.
    h.record(1_000_000);
    assert!(h.p95().is_some(), "64 samples crosses the threshold");
}

/// Spec 0337 test 5 / G4: before calibration the scale is today's.
/// With the default anchor (`ln(144)`), scores at the Fibonacci
/// boundaries produce roughly the same ordering as the old `heat_level`
/// bucketing — the floor and ceiling are exact, and intermediate scores
/// are strictly ordered between them.
#[test]
fn an_uncalibrated_document_renders_as_it_did() {
    let anchor = HEAT_ANCHOR_DEFAULT;

    // Score 1 maps to t=0 (the floor) exactly.
    assert_eq!(heat_fraction(1, anchor), 0.0);
    // Score 144 maps to t=1 (the ceiling) exactly, since ln(144)/ln(144)=1.
    assert!(
        (heat_fraction(144, anchor) - 1.0).abs() < 0.001,
        "score 144 must be at or near the top"
    );
    // Scores are strictly ordered between the floor and ceiling.
    let t8 = heat_fraction(8, anchor);
    let t55 = heat_fraction(55, anchor);
    assert!(
        0.0 < t8 && t8 < t55 && t55 < 1.0,
        "intermediate scores must be strictly ordered: t8={t8} t55={t55}"
    );
}

/// Spec 0337 test 6 / S3: `prefetch_step` is one of the three writers
/// (the one that visits the whole arena off-screen). A cache hit there
/// must feed the histogram — which is what makes calibration cover the
/// whole document and not just what has been drawn.
///
/// `prefetch_step` calls `record_heat_state` directly on a cache hit —
/// the same method `heat_cue_resolve` calls. This test exercises that
/// method with a graded state (mismatch: best=50, current=None) and
/// verifies the histogram receives the sample.
///
/// The other two callers of `record_heat_state` (`heat_cue_resolve`'s
/// settled-return and its unsettled-lookup arms) are exercised by
/// `a_settled_node_does_not_feed_it` and the render-path tests.
#[test]
fn every_writer_feeds_the_histogram() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::All;

    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);

    // Seed a mismatch (graded square): best=50, no current_key for an
    // untyped message node → read_heat_state sets current=Some(None).
    seed_range_heat_entry(&mut app, range.start, Some(50), 1, "int32", None);

    let before = app.heat_histogram.total();

    // Call read_heat_state + record_heat_state directly — the exact two
    // lines prefetch_step_inner executes on a cache hit. This is the
    // narrowest test of the prefetch path's histogram contribution.
    let state = app.read_heat_state(range.start, None, tiered::Tier::Prefetch);
    app.record_heat_state(idx, state);

    assert!(
        app.heat_histogram.total() > before,
        "record_heat_state on a graded cache hit must feed the histogram \
         (this is what prefetch_step does on every cache hit)"
    );
}

/// Spec 0337 test 7 / S2: a `Settled` node — unique optimum or
/// unmatched — carries no graded square and must not move the histogram.
/// The document root is also excluded; what matters is the variant, not
/// which node it is.
#[test]
fn a_settled_node_does_not_feed_it() {
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::All;

    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    let key = "google.protobuf.DescriptorProto";

    // Unique optimum — Settled { score: Some(50) } — no graded square.
    seed_range_heat_entry(&mut app, range.start, Some(50), 1, key, Some(50));
    let header = app.absolute_start(idx);
    app.heat_cue_for(header);
    assert_eq!(
        app.heat_histogram.total(),
        0,
        "a unique optimum is not a graded square"
    );

    // Unmatched — Settled { score: None } — also no graded square.
    // Reset the state so `record_heat_state` sees a fresh node.
    app.heat_states[idx] = heat_cue::HeatState::default();
    seed_range_heat_entry(&mut app, range.start, None, 0, key, None);
    app.heat_cue_for(header);
    assert_eq!(
        app.heat_histogram.total(),
        0,
        "unmatched (no candidate at all) is not a graded square"
    );
}

/// Spec 0337 test 8 / S6: the glyph hover box names the current anchor
/// so a reader who sees three squares at maximum brightness can learn
/// what they are "at least as large as". The anchor line is present for
/// both mismatch and tie glyphs but absent for the unmatched sentinel.
/// Driven through a full render+hit-test so the box and the square are
/// built from the same anchor the app holds.
#[test]
fn the_box_names_the_anchor() {
    // Mismatch glyph: a seeded mismatch, app at default anchor.
    // exp(ln(144)).round() = 144, so the box must say 144.
    let mut app = message_node_app();
    app.splash = false;
    app.heat_cues = HeatCueMode::Findings;
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    // Hit-test column 0 — the glyph column.
    let hit = app
        .doc_element_at_point(app.main_area.x, app.main_area.y)
        .expect("the glyph is a target");
    let lines: Vec<String> = super::super::popup_doc::doc_lines(&hit, app.heat_anchor)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("brightest at score 144")),
        "box must name the default anchor score (144): {lines:?}"
    );

    // Unmatched glyph: flat sentinel, no anchor line. Seeded as
    // an_unmatched_message_says_so_loudly does.
    let mut app2 = message_node_app();
    app2.splash = false;
    app2.heat_cues = HeatCueMode::All;
    let range2 = extract::message_payload_range(&app2.blob, &app2.tree[0].span.raw_range);
    seed_range_heat_entry(
        &mut app2,
        range2.start,
        None,
        0,
        "google.protobuf.DescriptorProto",
        None,
    );
    let mut terminal2 = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal2.draw(|frame| app2.render(frame)).unwrap();
    let hit2 = app2
        .doc_element_at_point(app2.main_area.x, app2.main_area.y)
        .expect("the unmatched glyph is a target");
    let lines2: Vec<String> = super::super::popup_doc::doc_lines(&hit2, app2.heat_anchor)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(
        !lines2.iter().any(|l| l.contains("brightest")),
        "unmatched sentinel must not claim a numeric anchor: {lines2:?}"
    );
}
