// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `PrefetchWalk`/`App::prefetch_step` (spec 0164 G7).

use super::super::heat_worker::{HeatRequest, HeatWorkerHandle, HEAT_REQUEST_QUEUE_MAX_ENTRIES};
use super::super::prefetch::PREFETCH_WALK_MAX_ROWS;
use super::super::tiered::Tier;
use super::super::*;
use super::heat_cue::seed_range_heat_entry;
use super::support::*;

// ---------------------------------------------------------------------
// PrefetchWalk::next_row — pure zigzag order. `PrefetchWalk` carries no
// visibility modifier (module-private to `tui`), but `tui::tests` is a
// descendant of `tui`, so its private fields/methods are directly
// reachable here without any test-only accessor.
// ---------------------------------------------------------------------

/// Successive calls alternate `cursor - 1`, `cursor + 1`, `cursor - 2`,
/// `cursor + 2`, ... — always the nearer of the two unexplored ends —
/// symmetric around the origin, so this pins the zigzag order itself
/// regardless of which end the walk eventually runs out on.
#[test]
fn next_row_zigzags_outward_nearest_first() {
    let mut walk = PrefetchWalk {
        origin_line: 5,
        above: 0,
        below: 0,
        above_done: false,
        below_done: false,
        above_pos: None,
        below_pos: None,
        structural_version: 0,
    };
    let mut order = Vec::new();
    while let Some(row) = walk.next_row(11) {
        order.push(row);
    }
    assert_eq!(order, vec![4, 6, 3, 7, 2, 8, 1, 9, 0, 10]);
}

/// Once one end (here, `above`, since the origin sits near the top)
/// runs out, the walk keeps advancing the still-open end instead of
/// stalling or repeating — "regardless of direction" from the spec's
/// G7 test-plan wording.
#[test]
fn next_row_keeps_walking_the_open_end_once_the_other_is_exhausted() {
    let mut walk = PrefetchWalk {
        origin_line: 1,
        above: 0,
        below: 0,
        above_done: false,
        below_done: false,
        above_pos: None,
        below_pos: None,
        structural_version: 0,
    };
    let mut order = Vec::new();
    while let Some(row) = walk.next_row(5) {
        order.push(row);
    }
    assert_eq!(order, vec![0, 2, 3, 4]);
}

// ---------------------------------------------------------------------
// Spec 0191 S1 — the row budget
// ---------------------------------------------------------------------

/// Test plan item 1. With both ends far from the document's edges, the
/// walk stops after exactly `PREFETCH_WALK_MAX_ROWS` rows rather than
/// sweeping to both ends, and stays stopped — this is the property that
/// lets `prefetch_step` report `Idle`, which is in turn what lets the
/// main thread reach `recv_timeout` instead of spinning.
#[test]
fn next_row_stops_after_the_row_budget_is_spent() {
    let mut walk = PrefetchWalk {
        origin_line: 100_000,
        above: 0,
        below: 0,
        above_done: false,
        below_done: false,
        above_pos: None,
        below_pos: None,
        structural_version: 0,
    };
    let mut visited = 0;
    while walk.next_row(200_000).is_some() {
        visited += 1;
        assert!(visited <= PREFETCH_WALK_MAX_ROWS, "must not overrun");
    }
    assert_eq!(visited, PREFETCH_WALK_MAX_ROWS);
    assert!(
        walk.next_row(200_000).is_none(),
        "an exhausted walk must stay exhausted"
    );
}

/// Test plan item 2. The budget is shared across both ends, not applied
/// per side: with the origin at row 0 the upward end is exhausted after
/// its very first attempt, and the walk must still spend its full
/// allowance downward instead of settling for half of it.
#[test]
fn the_row_budget_is_shared_across_both_ends() {
    let mut walk = PrefetchWalk {
        origin_line: 0,
        above: 0,
        below: 0,
        above_done: false,
        below_done: false,
        above_pos: None,
        below_pos: None,
        structural_version: 0,
    };
    let mut rows = Vec::new();
    while let Some(row) = walk.next_row(200_000) {
        rows.push(row);
    }
    assert_eq!(rows.len(), PREFETCH_WALK_MAX_ROWS);
    assert_eq!(rows.first(), Some(&1), "origin at 0 can only walk downward");
    assert_eq!(rows.last(), Some(&PREFETCH_WALK_MAX_ROWS));
}

// ---------------------------------------------------------------------
// App::prefetch_step
// ---------------------------------------------------------------------

/// End-to-end: a full walk visits every eligible sibling *other than
/// the cursor's own row* exactly once (one `Tier::Prefetch` push per
/// call, per node — the walk starts at `cursor - 1`/`cursor + 1`, so
/// the origin row itself is never a candidate), then goes `Idle` and
/// stays `Idle` on further calls without pushing again.
#[test]
fn prefetch_step_walks_every_eligible_node_once_then_goes_idle() {
    let mut app = wide_sibling_scalars_app(5);
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.set_cursor(2);

    let mut progressed = 0;
    while let PrefetchStep::Progressed = app.prefetch_step() {
        progressed += 1;
        assert!(progressed <= 4, "must not loop forever");
    }
    assert_eq!(
        progressed, 4,
        "every sibling other than the cursor's own row must be visited exactly once"
    );
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 4);

    assert!(matches!(app.prefetch_step(), PrefetchStep::Idle));
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        4,
        "an exhausted walk must not push again"
    );
}

/// G6: once the request queue is saturated with entries at or above
/// `Visible` tier, a `Prefetch` push has nothing at or below its own
/// tier to evict and is `Rejected` — `prefetch_step` must treat that
/// as "stop the whole walk", so the *next* call reports `Idle`
/// immediately, with no further push.
#[test]
fn a_rejected_push_makes_the_next_call_idle_with_no_further_push() {
    let mut app = wide_sibling_scalars_app(2);
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.set_cursor(0);

    // Saturate the queue with `User`-tier entries at keys far outside
    // any node's own payload range, so the probe pushes below can
    // never collide with (and merge into) one of them.
    for i in 0..HEAT_REQUEST_QUEUE_MAX_ENTRIES {
        let key = 1_000_000 + i;
        app.heat_worker.as_ref().unwrap().push(
            HeatRequest {
                range: key..key + 1,
                current_key: None,
                start: 0,
                end: heat_cue::HEAT_CUE_PREVIEW,
                tier: Tier::User,
            },
            Tier::User,
        );
    }
    let before = app.heat_worker.as_ref().unwrap().queue_len();
    assert_eq!(before, HEAT_REQUEST_QUEUE_MAX_ENTRIES);

    assert!(matches!(app.prefetch_step(), PrefetchStep::Idle));
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        before,
        "a rejected push must not grow the queue"
    );
    assert!(matches!(app.prefetch_step(), PrefetchStep::Idle));
}

/// Moving the cursor between calls restarts the walk from the new
/// origin — `prefetch_walk.origin_line` tracks the new cursor's
/// display row, not the old one.
///
/// The old wave's entry is still *in* the queue afterwards: the
/// restart is an O(1) splice on the UI thread, not a walk (spec 0189
/// G2). What changed with 0189 is that the entry is no longer
/// reachable by `pop_highest`, so the worker reclaims it rather than
/// spending a `score_all` on a range ranked from an origin that no
/// longer exists. That reclamation is the worker's job and is covered
/// by `pop_blocking_discards_a_superseded_wave_instead_of_serving_it`.
#[test]
fn changing_cursor_restarts_the_walk_and_supersedes_the_old_wave() {
    let mut app = wide_sibling_scalars_app(5);
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.set_cursor(2);

    assert!(matches!(app.prefetch_step(), PrefetchStep::Progressed));
    let origin_after_first = app.prefetch_walk.origin_line;
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);

    app.set_cursor(4);
    assert!(matches!(app.prefetch_step(), PrefetchStep::Progressed));
    assert_ne!(
        app.prefetch_walk.origin_line, origin_after_first,
        "the walk must restart from the new cursor's row"
    );
    assert_eq!(
        app.prefetch_walk.structural_version, app.structural_version,
        "a fresh walk must record the current structural_version"
    );
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        2,
        "the restart must not walk the old wave — it is spliced aside, \
         and the worker reclaims it"
    );
}

/// Spec 0191 test plan item 4, and the whole point of the budget: on a
/// document with more eligible rows than the walk may spend, the wave
/// ends anyway. `Idle` is what breaks `run_loop` out of its
/// `PrefetchStep::Progressed => continue` branch and onto
/// `recv_timeout`, so without this the main thread never blocks.
///
/// The walk — not the request queue — must be what stops it. Both caps
/// hold 2048 today, so a saturated queue would produce the same
/// `Progressed` count; asserting the walk's own exhausted state is what
/// distinguishes the two.
#[test]
fn prefetch_step_stops_after_the_row_budget_even_with_rows_to_spare() {
    let mut app = wide_sibling_scalars_app(PREFETCH_WALK_MAX_ROWS + 100);
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.set_cursor(PREFETCH_WALK_MAX_ROWS / 2);

    let mut progressed = 0;
    while let PrefetchStep::Progressed = app.prefetch_step() {
        progressed += 1;
        assert!(
            progressed <= PREFETCH_WALK_MAX_ROWS,
            "the walk must not outrun its budget"
        );
    }
    assert_eq!(progressed, PREFETCH_WALK_MAX_ROWS);
    assert!(
        app.prefetch_walk.above_done && app.prefetch_walk.below_done,
        "the walk itself must be what ended the wave, not a full queue"
    );
    assert!(matches!(app.prefetch_step(), PrefetchStep::Idle));
}

/// Spec 0224 S3: the walk maintains its own skip. A node whose answer
/// is already in `heat_caches` costs the wave one step to discover —
/// and exactly one, ever, because that step records the `HeatState` it
/// just proved exists. Without the write-back the node would be
/// re-proved on every subsequent wave, so the read-ahead would advance
/// more slowly the more the worker had answered.
///
/// The queue must stay empty throughout: a hit is the whole point, and
/// a push would mean the seeded entry did not cover the ask.
#[test]
fn a_prefetch_cache_hit_settles_the_node_for_the_next_wave() {
    let mut app = wide_sibling_scalars_app(5);
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.set_cursor(2);

    // The zigzag's first target is `cursor - 1`. Seed the answer for
    // that node only, so the hit and the miss are distinguishable.
    let (first, _) = app
        .prev_visible(LinePos::header(app.cursor))
        .expect("the cursor is not the first row");
    let idx = first.node;
    let range = app.heat_scored_range(idx);
    let key = app
        .current_type_key(idx)
        .expect("a scalar sibling has a current type");
    seed_range_heat_entry(&mut app, range.start, Some(50), 1, &key, Some(10));
    assert!(!app.heat_states[idx].settled());

    assert!(matches!(app.prefetch_step(), PrefetchStep::Progressed));
    assert!(
        app.heat_states[idx].settled(),
        "a cache hit must record the state it just proved exists"
    );
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        0,
        "a covered node must not be pushed"
    );

    // The following step is spent on a different node, not on
    // re-proving this one.
    assert!(matches!(app.prefetch_step(), PrefetchStep::Progressed));
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);
}

/// No worker running at all: `prefetch_step` is `Idle` immediately —
/// nothing to prefetch into.
#[test]
fn prefetch_step_is_idle_without_a_worker() {
    let mut app = sibling_leaves_app(&["a", "b"]);
    assert!(app.heat_worker.is_none());
    app.set_cursor(0);

    assert!(matches!(app.prefetch_step(), PrefetchStep::Idle));
}
