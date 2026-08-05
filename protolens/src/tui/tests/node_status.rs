// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0247: the status each node rolls up from its subtree.
//!
//! The classifier that reads one row is tested next to itself in
//! `crate::node_status`; what is checked here is the roll-up, and that
//! the two view toggles which change what a row *looks* like change no
//! node's status.
//!
//! The incremental update has no test of its own on purpose: spec 0247
//! S13 hangs `assert_status_is_exact` off `finalize_override_batch`, so
//! every splice in the whole suite already compares the incrementally
//! maintained arrays against a full rebuild.

use super::support::*;
use crate::node_status::Status;

/// Spec 0247 S7: the roll-up is `max`, so one defective leaf decides
/// its parent — and the sibling that is fine does not undo it.
#[test]
fn the_worst_child_wins() {
    let (app, inner, known, unknown) = unknown_field_fixture();

    assert_eq!(app.status_of(known), Status::Ok);
    assert_eq!(
        app.status_of(unknown),
        Status::Unknown,
        "a row keyed by a field number is a field no schema declares"
    );
    assert_eq!(
        app.status_own[inner],
        Status::Ok,
        "`inner {{` says nothing wrong about itself"
    );
    assert_eq!(
        app.status_of(inner),
        Status::Unknown,
        "but it must carry its worst child's news"
    );
}

/// Spec 0247 G2: the news reaches the root, which is the whole point —
/// a fold toggle at the top has to be able to say that something is
/// wrong far below it.
#[test]
fn a_defect_reaches_every_node_above_it() {
    let (app, inner, _, unknown) = unknown_field_fixture();

    let mut cur = unknown;
    while let Some(parent) = app.parent(cur) {
        assert_eq!(
            app.status_of(parent),
            Status::Unknown,
            "node {parent} is above the defect and must show it"
        );
        cur = parent;
    }
    assert_ne!(cur, inner, "the fixture must nest deeper than one level");
}

/// Spec 0247 S12: `a` strips annotations at display time only, so no
/// node's status may move. The rung the fixture sits on is the one that
/// makes this a real question — an unknown field is read off the *key*,
/// which `a` never touches, while the two anomaly rungs are read off
/// the annotation, which it hides.
#[test]
fn hiding_annotations_does_not_change_any_status() {
    let (mut app, ..) = unknown_field_fixture();
    let before = app.status_rolled.clone();

    app.annotations = false;
    app.rebuild_status();

    assert_eq!(app.status_rolled, before);
}

/// Spec 0247 S11: a fold toggle colored by its subtree is only useful
/// if folding does not change the answer — the color is what tells you
/// whether to unfold at all.
#[test]
fn folding_does_not_change_any_status() {
    let (mut app, inner, ..) = unknown_field_fixture();
    let before = app.status_rolled.clone();

    app.toggle_fold(inner);
    app.rebuild_status();

    assert_eq!(app.status_rolled, before);
}

/// Spec 0247 S9: naming a field is what clears the blue, and it needs
/// no rule of its own — the override rewrites the document, the row
/// stops showing a number, and the status simply reads what is there.
#[test]
fn naming_a_field_clears_the_unknown() {
    let (mut app, inner, _, unknown) = unknown_field_fixture();
    assert_eq!(app.status_of(unknown), Status::Unknown);

    app.run_command("override test.Inner:9 --field-name tally");

    assert_eq!(app.field_name_for(unknown), "tally");
    assert_eq!(app.status_of(unknown), Status::Ok);
    assert_eq!(
        app.status_of(inner),
        Status::Ok,
        "and the parent must lose the news too"
    );
}
