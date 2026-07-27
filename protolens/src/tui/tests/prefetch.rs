// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `PrefetchWalk`/`App::prefetch_step` (spec 0164 G7).

use super::super::heat_worker::{HeatRequest, HeatWorkerHandle, HEAT_REQUEST_QUEUE_MAX_ENTRIES};
use super::super::tiered::Tier;
use super::super::*;
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
        structural_version: 0,
    };
    let mut order = Vec::new();
    while let Some(row) = walk.next_row(5) {
        order.push(row);
    }
    assert_eq!(order, vec![0, 2, 3, 4]);
}

/// `n` document-order sibling scalar (`WT_VARINT`) fields, one line
/// each, each backed by a real 2-byte tag+value encoding in `blob` —
/// unlike `sibling_leaves_app`'s synthetic, unbacked ranges, this
/// fixture's `raw_range`s are real slices of a non-empty blob, needed
/// since `prefetch_step` runs every candidate through `extract::
/// message_payload_range`, which indexes into `blob` at `raw_range.
/// start` and panics on an out-of-bounds/empty one.
fn prefetch_fixture(n: usize) -> App {
    let lines: Vec<String> = (0..n).map(|i| format!("field_{i}: 0")).collect();
    let mut blob = Vec::new();
    let tree: Vec<TreeNode> = (0..n)
        .map(|i| {
            let start = blob.len();
            blob.push((((i as u64 + 1) << 3) | WT_VARINT as u64) as u8);
            blob.push(0);
            TreeNode {
                span: NodeSpan {
                    field_number: i as u64 + 1,
                    raw_range: start..start + 2,
                    text_range: i..i + 1,
                    level: 0,
                    type_fqdn: None,
                    is_message: false,
                    packed_record_start: None,
                    wire_type: WT_VARINT,
                },
                parent: None,
                first_child: None,
                last_child: None,
                next_sibling: (i + 1 < n).then_some(i + 1),
                prev_sibling: i.checked_sub(1),
                doc_next: (i + 1 < n).then_some(i + 1),
                doc_prev: i.checked_sub(1),
                rendered_as: None,
            }
        })
        .collect();
    let decoded = Decoded {
        lines,
        tree,
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        blob,
        wrapper_offset: 0,
        root_candidates: Vec::new(),
    };
    App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    )
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
    let mut app = prefetch_fixture(5);
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
    let mut app = prefetch_fixture(2);
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
/// display row, not the old one — and the old wave's own entry
/// survives the restart's `start_new_wave()` call (demoted to
/// `prefetch_previous`, not discarded): the queue ends up holding
/// both the old and the new entry, not just the new one.
#[test]
fn changing_cursor_restarts_the_walk_without_discarding_the_old_wave() {
    let mut app = prefetch_fixture(5);
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
        "the old wave's entry must survive the restart, not be discarded"
    );
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
