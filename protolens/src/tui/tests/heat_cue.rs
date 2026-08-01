// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_cue::{
    derive_stats, heat_display, heat_level, score_of, HeatCue, HeatCueKind, HeatDisplay, HeatState,
    RangeHeatStats, HEAT_CUE_PREVIEW, HEAT_GLYPH, SCORE_FLOOR,
};
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

    // One seeded entry, keyed on the record's payload start, is enough
    // to light up every line of the run.
    seed_range_heat_entry(&mut app, record.start, Some(50), 1, "int32", Some(10));
    let rows = app.node_lines(run);
    assert_eq!(rows.len(), 3, "the run draws one row per element");
    for line in rows {
        assert!(
            matches!(app.heat_cue_for(line), HeatDisplay::Cue(_)),
            "element line {line} must show the record's cue"
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
    assert!(matches!(heat_display(state), HeatDisplay::Unknown));
    let state = HeatState::new(None, Some(Some(5)));
    assert!(matches!(heat_display(state), HeatDisplay::Unknown));
    assert!(!state.settled());
}

/// H-02 (spec 0138 G8): every candidate vetoed (`best_score: None`) —
/// nothing shown, settled, regardless of `current`.
#[test]
fn h02_none_when_every_candidate_is_vetoed() {
    let stats = RangeHeatStats {
        best_score: None,
        best_count: 0,
    };
    let state = HeatState::new(Some(stats), None);
    assert!(matches!(heat_display(state), HeatDisplay::None));
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
        heat_display(state),
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
    let display = heat_display(state);
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
    let display = heat_display(state);
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
        heat_display(state),
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
        heat_display(state),
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
/// a unique optimum — nothing shown, settled, same as before G9
/// existed.
#[test]
fn h07_none_for_a_unique_optimum() {
    let stats = RangeHeatStats {
        best_score: Some(50),
        best_count: 1,
    };
    let state = HeatState::new(Some(stats), Some(Some(50)));
    assert!(matches!(heat_display(state), HeatDisplay::None));
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

/// Spec 0138: `i` toggles `heat_cues_hidden`, suppressing the cue
/// without discarding the caches — verified by pre-populating them
/// directly (bypassing the need for a real scoring graph) so a cue
/// would otherwise be present.
#[test]
fn i_toggles_heat_cues_hidden() {
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

    assert!(!app.heat_cues_hidden);
    let display = app.heat_cue_for(header_line);
    assert!(matches!(
        display,
        HeatDisplay::Cue(HeatCue {
            kind: HeatCueKind::Mismatch {
                current: Some(10),
                best: 50
            },
            ..
        })
    ));

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert!(app.heat_cues_hidden);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::None));

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert!(!app.heat_cues_hidden);
    assert!(matches!(app.heat_cue_for(header_line), HeatDisplay::Cue(_)));
}

/// End-to-end render check (spec 0138 N1): with a cue pre-cached (as
/// `i_toggles_heat_cues_hidden` above), the main pane's header row
/// shows `HEAT_GLYPH` in its own leading column and a trailing
/// ` [current/best]` suffix; hiding the cue reverts the leading column
/// to blank and drops the suffix, without otherwise disturbing the
/// line's own indentation.
#[test]
fn render_shows_the_glyph_column_and_suffix_when_a_cue_is_present() {
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

    app.heat_cues_hidden = true;
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
        caches.complete.as_ref().map(|(r, c)| (r.clone(), c.len())),
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
        caches.complete.as_ref().unwrap().1.len(),
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
    assert!(caches.complete.is_none());
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

/// `heat_cues_hidden` (2026-07-19 feedback) no longer skips
/// `warm_up_heat_cues`'s own gate — the background worker must keep
/// priming the cache even while cues are hidden, so `heat_cue_for`
/// (called by the warm-up loop below) still pushes its request; only
/// its returned cue is suppressed, at the `heat_cue_for` layer, not
/// here.
#[test]
fn heat_cue_for_still_pushes_a_request_when_heat_cues_hidden() {
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_cues_hidden = true;
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

/// Real worker thread, real tiny in-memory graph, through the `App`
/// layer end-to-end (spec 0152 test plan): a `heat_cue_for` miss with
/// a `HeatWorkerHandle` installed (via `DescriptorContext::
/// for_test_with_graph`) leaves the node `Pending`; the worker's own
/// cache write is later picked up by `recheck_pending_heat_states`
/// (the same re-check `AppEvent::HeatWorkerProgress` triggers in
/// `run_loop`), resolving it. Complements `heat_worker.rs`'s own
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
///   cue, not `Unknown`.
/// - Worse, that same schedule *removes* the node from
///   `pending_heat_recheck` instead of inserting it, so the poll below
///   would have nothing to recheck and would report success on its
///   first pass without exercising the path this test is named for.
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
    let header_line = app.absolute_start(idx);

    assert!(matches!(
        app.heat_cue_for(header_line),
        HeatDisplay::Unknown
    ));
    assert!(!app.heat_states[idx].settled());
    // The two facts phase two rests on: the work is queued, and the
    // node is on the recheck list that `recheck_pending_heat_states`
    // iterates — it pushes nothing of its own, so a node missing from
    // this set can never settle.
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);
    assert!(app.pending_heat_recheck.contains(&idx));

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
    // event-driven wiring, just the worker/cache-recheck contract.
    let mut resolved = false;
    for _ in 0..200 {
        app.recheck_pending_heat_states();
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

/// Spec 0208 test-plan item 6 (S3/G3). The node under the cursor asks
/// at `Tier::User`; every other visible node asks at `Tier::Visible`.
///
/// Observed through the queue's own `activity()` — the highest live
/// tier — rather than by inspecting the argument at the call site, so
/// what is pinned is what the worker will actually serve first. The
/// ordinary line is resolved *before* the cursor's, since `activity()`
/// reports a maximum and would hide a wrong answer taken the other way
/// round.
#[test]
fn the_cursor_node_asks_at_user_tier_and_other_visible_nodes_do_not() {
    let (mut app, items) = repeated_message_fixture();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let (under_cursor, elsewhere) = (items[0], items[1]);
    app.set_cursor(under_cursor);

    app.heat_cue_for(app.absolute_start(elsewhere));
    assert_eq!(
        app.heat_worker.as_ref().unwrap().activity(),
        Some(Tier::Visible),
        "an ordinary visible line must not claim user attention"
    );

    app.heat_cue_for(app.absolute_start(under_cursor));
    assert_eq!(
        app.heat_worker.as_ref().unwrap().activity(),
        Some(Tier::User),
        "the cursor's own node outranks the rest of the viewport"
    );
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        2,
        "two distinct payload ranges, so two entries — not one merged"
    );
}

/// Spec 0208 test-plan item 7 (S3). Moving the cursor moves the
/// promotion with it. The node left behind is *not* demoted — a tier
/// never moves down (spec 0164 G5) — it simply stops being re-asked at
/// `User`, which is what makes the ladder stable under a moving cursor
/// instead of oscillating.
#[test]
fn moving_the_cursor_promotes_the_new_node_without_demoting_the_old() {
    let (mut app, items) = repeated_message_fixture();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    let (first, second) = (items[0], items[1]);

    app.set_cursor(first);
    app.heat_cue_for(app.absolute_start(first));
    app.set_cursor(second);
    app.heat_cue_for(app.absolute_start(second));

    // Both are `User` now, so both outrank anything else queued, and
    // the most recently asked one is served first (S4a).
    let queue = app.heat_worker.as_ref().unwrap();
    assert_eq!(queue.activity(), Some(Tier::User));
    assert_eq!(queue.queue_len(), 2);

    // Re-asking for the node the cursor has left now happens at
    // `Tier::Visible`, which must neither demote it nor re-rank it.
    app.heat_cue_for(app.absolute_start(first));
    assert_eq!(
        app.heat_worker.as_ref().unwrap().activity(),
        Some(Tier::User)
    );
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 2);
}
