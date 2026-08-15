// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `splice_override` and the `render_overrides` batch: what a retype does
//! to the tree and to the line buffer.
//!
//! The file's other three subjects moved out — `resolve_export_fields`,
//! the preview byte budget, and `materialize_line_patches` — each to a
//! sibling named after the thing it tests.

use super::super::*;
use super::support::*;
use std::collections::HashMap;
/// Spec 0118 §4: `splice_override` regenerates a whole node (not just
/// its interior) into `self.document_lines()`/`self.tree`, repeatable on the
/// same node (the design's key risk: post-order array contiguity does
/// not survive a *second* override of the same node, since the first
/// override's new nodes are appended at the array's end —
/// `splice_override` must never rely on it).
#[test]
fn apply_override_splices_tree_and_lines_repeatedly() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto3_fds(
        "test_apply_override.proto",
        vec![
            message(
                "Outer",
                vec![field_of(
                    "inner",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Node",
                )],
            ),
            message(
                "Node",
                vec![
                    field_of("a", 1, Label::Optional, Type::Message, ".test.Leaf"),
                    field("b", 2, Label::Optional, Type::Int32),
                ],
            ),
            message("Leaf", vec![field("val", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // Outer wraps Node (field 1, LEN), whose payload is
    // a = Leaf { val: 9 } (field 1, LEN) then b = 42 (field 2, varint).
    let mut app = fixture_under(
        "apply-override",
        &fds,
        "test.Outer",
        &[
            0x0Au8, 0x06, //
            0x0A, 0x02, 0x08, 0x09, //
            0x10, 0x2A,
        ],
    );

    let node_idx =
        node_with_type(&app, "test.Node").expect("tree must contain the Node submessage");
    let node_level = app.tree[node_idx].span.level;

    // Fold the "a" child before overriding, to verify the stale-fold
    // scrubbing (`collect_descendants` cleanup).
    let a_idx_before = app
        .first_child(node_idx)
        .expect("Node has at least one child");
    // Spec 0210 S2: through `refresh_line_counts`, since a fold now moves
    // the line counters the row walk reads.
    app.folded.insert(a_idx_before);
    app.refresh_line_counts(a_idx_before);

    let assert_children = |app: &App, tag: &str| {
        let mut children = Vec::new();
        let mut cur = app.first_child(node_idx);
        while let Some(c) = cur {
            children.push(c);
            cur = app.next_sibling(c);
        }
        assert_eq!(children.len(), 2, "{tag}: expected two children (a, b)");
        for &c in &children {
            assert_eq!(
                app.tree[c].span.level,
                node_level + 1,
                "{tag}: child level must match pre-override nesting"
            );
        }
        assert_eq!(
            type_name_of(app, children[0]),
            Some("test.Leaf"),
            "{tag}: first child must resolve to test.Leaf"
        );
    };

    app.override_target = Some(node_idx);

    // 1) Re-typed as itself: idempotent structural round-trip.
    app.splice_override(node_idx, Some("test.Node".to_string()), None)
        .expect("re-typing as the same type must succeed");
    assert_children(&app, "re-typed as itself");
    assert_eq!(type_name_of(&app, node_idx), Some("test.Node"));
    assert!(
        !app.folded.contains(&a_idx_before),
        "orphaned old child must be scrubbed from `folded`"
    );

    // 2) Raw override (no schema).
    app.splice_override(node_idx, None, None)
        .expect("raw override must succeed");
    assert_eq!(app.tree[node_idx].span.type_fqdn, NO_FQDN);

    // 3) Re-typed again, on top of two prior overrides — exercises
    // repeated overrides of the same node.
    app.splice_override(node_idx, Some("test.Node".to_string()), None)
        .expect("third override must still succeed");
    assert_children(&app, "re-typed a third time");

    // Regression (2026-07-19 crash report): `splice_override` appends
    // fresh nodes to `app.tree` on every call but used to leave
    // `heat_states` at its original `App::new`-time length, so
    // `heat_cue_for` on one of those freshly-pushed nodes indexed past
    // the end and panicked. `heat_states` must stay parallel to `tree`,
    // and calling `heat_cue_for` for every line must not panic.
    assert_eq!(
        app.heat_states.len(),
        app.tree.len(),
        "heat_states must stay parallel to tree after repeated splices"
    );
    for line in 0..app.document_lines().len() {
        app.heat_cue_for(line);
    }

    // Spec 0210 S2: line ownership must stay fully consistent with the
    // doc chain. Every node reachable via `doc_next` from `first_node`
    // owns its own header line, and nothing else does — asked of the
    // counters, since no line-keyed map holds the answer.
    let mut owners: Vec<Option<u32>> = vec![None; app.document_lines().len()];
    let mut count = 0;
    let mut cur = Some(app.first_node);
    while let Some(c) = cur {
        let line = app.absolute_start(c);
        assert_eq!(
            app.line_pos(line).map(|p| (p.node, p.line_in_node)),
            Some((c, 0)),
            "line {line} must resolve to node {c}'s header"
        );
        owners[line] = Some(c as u32);
        count += 1;
        assert!(count <= app.tree.len(), "doc chain must not cycle");
        cur = app.doc_next(c);
    }
    for (line, owner) in owners.iter().enumerate() {
        if owner.is_none() {
            // Spec 0216 S7: a node's non-header rows are its closing
            // brace if it is bracketed, and the rest of a packed run's
            // elements if it is not.
            assert!(
                app.line_pos(line).is_some_and(|p| p.line_in_node > 0),
                "line {line} is no node's header, so it must be a later \
                 row of one"
            );
        }
    }
}

/// Overriding a plain scalar (string) field into an incompatible
/// message type must not panic — it should apply the override and
/// surface the mismatch as ordinary `TYPE_MISMATCH`/`INVALID_*`
/// annotations in the interior, exactly like any other malformed
/// nested-message re-decode (feedback, 2026-07-16: `t` used to panic
/// here, in the header-patching path spec 0135 has since deleted).
#[test]
fn splice_override_on_an_incompatible_scalar_does_not_panic() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto3_fds_in(
        "incompat",
        "incompat.proto",
        vec![
            message(
                "StrHolder",
                vec![field("s", 1, Label::Optional, Type::String)],
            ),
            message("Target", vec![field("id", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // StrHolder { s: "hello" } — field 1 (tag 0x0A, LEN), 5 bytes.
    let mut app = fixture_under(
        "incompat-override",
        &fds,
        "incompat.StrHolder",
        b"\x0A\x05hello",
    );

    let s_idx = app.nth_child(app.first_node, 0).expect("must find field 1");
    assert!(
        app.can_override(s_idx),
        "a WT_LEN scalar must be overridable"
    );

    app.splice_override(s_idx, Some("incompat.Target".to_string()), None)
        .expect("override onto an incompatible type must still succeed");
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("INVALID") || l.contains("TYPE_MISMATCH")),
        "mismatch must surface as an inline annotation, not a panic: {:?}",
        app.document_lines()
    );
}

/// Spec 0184 G1: a packed run's elements must not be separately
/// numbered children, because committing an override on the run
/// collapsed them into one node — which used to renumber every later
/// sibling and silently re-point any path recorded before the commit.
///
/// Spec 0216 S22 removes the collapse by never splitting in the first
/// place: the run is one slot before the override as well as after, so
/// there is no count left for a commit to change.
#[test]
fn overriding_a_packed_run_does_not_renumber_later_siblings() {
    let (mut app, run, tail, _a, _b) = packed_run_with_tail_fixture();

    // The paths a user's override entries would have recorded, before
    // the packed run is touched at all.
    let before: Vec<String> = [tail, _a, _b]
        .iter()
        .map(|&i| app.positional_path(i))
        .collect();
    assert_eq!(before, ["/2", "/3", "/4"]);
    for (&idx, path) in [tail, _a, _b].iter().zip(&before) {
        assert_eq!(
            app.resolve_path(path),
            Some(idx),
            "precondition: {path} designates the node it was taken from"
        );
    }

    app.override_target = Some(run);
    app.splice_override(run, None, None)
        .expect("raw override on a packed element must succeed");

    for (&idx, path) in [tail, _a, _b].iter().zip(&before) {
        assert_eq!(
            app.resolve_path(path),
            Some(idx),
            "a path recorded before the override must still designate \
             the same node afterwards"
        );
        assert_eq!(app.positional_path(idx), *path);
    }
}

/// A packed run overridden as `None` and then deleted must render as
/// the packed run again — regression for a bug where it stuck at
/// `1: "\004\000\002\000"  #@ bytes; TYPE_MISMATCH`. `register_wrapper`
/// declared its synthetic field `optional`, which cannot carry a
/// LEN-framed record, and the `None` render had already cleared the
/// node's `packed_record_start`, so nothing was left to say the record
/// was a packed run. Retyping a live run to a packable primitive was
/// broken the same way, and is checked here too so the fix cannot
/// regress to reading only `packed_record_start`.
#[test]
fn a_deleted_override_restores_a_packed_run_rather_than_a_type_mismatch() {
    let (mut app, run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let before = app.document_lines().clone();
    assert!(
        before[1].contains("repeated int32 [packed=true]"),
        "fixture must start as a packed run: {before:?}"
    );

    let origin = OverrideOrigin::Path {
        path: app.positional_path(run),
    };
    app.overrides
        .activate(origin.clone(), Some("none".to_string()));
    app.render_overrides(app.first_node);

    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("the entry just activated must be there to delete");
    app.overrides.remove(entry_idx);
    app.render_overrides(app.first_node);
    assert_eq!(
        app.document_lines(),
        before,
        "deleting the override must restore the packed run verbatim"
    );

    let (mut retyped, run, _, _, _) = packed_run_with_tail_fixture();
    retyped
        .splice_override(run, Some("int32".to_string()), None)
        .expect("retyping a packed run to its own element type must succeed");
    assert_eq!(
        retyped.document_lines(),
        before,
        "an explicit retype to a packable primitive must render the run, not a mismatch"
    );
}

/// Spec 0251 S9: the render cache serves the live preview and nothing
/// else, so a workload that visits the whole document must leave it
/// exactly as it found it. This is what keeps the 8 MB budget a budget
/// for one node at the cursor rather than a share of the document —
/// before S5 a full-document pass inserted an entry per spliced node
/// and evicted whatever the user was actually looking at.
///
/// `len()` is a faithful witness of "wrote nothing" here: nothing can
/// shrink it (only an insert evicts) and a sweep's keys carry
/// `is_preview == false`, so they can never collide with the primed
/// preview entries and be absorbed into them.
#[test]
fn a_sweep_writes_to_no_cache() {
    let (mut app, run, tail, _a, _b) = packed_run_with_tail_fixture();

    // Prime with two previews, so the assertion below is "unchanged"
    // rather than the weaker "still empty".
    app.render_node_as(run, Some("int32"), true, None)
        .expect("a preview of the packed run must render");
    app.render_node_as(tail, Some("test.Inner"), true, None)
        .expect("a preview of the tail must render");
    assert_eq!(app.render_cache.len(), 2, "both previews must be cached");

    // A full-document pass that really splices: activating an override
    // on the run makes `resettle_node` re-render it and everything the
    // walk revisits. Asserted, so the test cannot pass by sweeping
    // nothing.
    let before = app.document_lines().clone();
    let origin = OverrideOrigin::Path {
        path: app.positional_path(run),
    };
    app.overrides.activate(origin.clone(), None);
    app.render_overrides(app.first_node);
    assert_ne!(
        app.document_lines(),
        before,
        "the sweep must have spliced something for this test to mean anything"
    );
    assert_eq!(
        app.render_cache.len(),
        2,
        "a sweep must not write to the render cache"
    );

    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("the entry must exist");
    app.overrides.toggle_active(entry_idx);
    app.render_overrides(app.first_node);
    assert_eq!(
        app.render_cache.len(),
        2,
        "the sweep that reverts the override must not write either"
    );
}

/// Spec 0184 test plan, "ordinal stability across override state": the
/// whole path map is identical before an override on a packed run,
/// while it is active, and after deactivating it. This is the property
/// the previous test checks pointwise, asserted over every live node
/// and across the full activate/deactivate cycle — the direction that
/// used to shift ordinals *back*.
#[test]
fn packed_run_ordinals_are_stable_across_the_override_lifecycle() {
    let (mut app, run, tail, a, b) = packed_run_with_tail_fixture();

    let path_map = |app: &App| -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut cur = Some(app.first_node);
        while let Some(i) = cur {
            out.push((i, app.positional_path(i)));
            cur = app.doc_next(i);
        }
        out
    };

    let baseline = path_map(&app);
    let watched: Vec<String> = [tail, a, b]
        .iter()
        .map(|&i| app.positional_path(i))
        .collect();

    let origin = OverrideOrigin::Path {
        path: app.positional_path(run),
    };
    app.overrides.activate(origin.clone(), None);
    app.render_overrides(app.first_node);
    let while_active: Vec<String> = [tail, a, b]
        .iter()
        .map(|&i| app.positional_path(i))
        .collect();
    assert_eq!(while_active, watched, "ordinals must not move on activate");

    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("the entry must exist");
    app.overrides.toggle_active(entry_idx);
    app.render_overrides(app.first_node);
    let after: Vec<String> = [tail, a, b]
        .iter()
        .map(|&i| app.positional_path(i))
        .collect();
    assert_eq!(after, watched, "ordinals must not move back on deactivate");

    // The nodes that were live at the start are still live, still in
    // document order, and still at the same paths.
    let final_map = path_map(&app);
    let baseline_paths: HashMap<usize, String> = baseline.into_iter().collect();
    for (idx, path) in final_map {
        if let Some(before) = baseline_paths.get(&idx) {
            assert_eq!(&path, before, "node {idx} changed path");
        }
    }
}

/// Spec 0184 S5: `positional_path`'s backward walk and
/// `render_overrides_inner`'s forward ordinal counter are two
/// independent implementations of the same rule, and a divergence
/// between them fails *silently* — an override stored under one path
/// and looked up under another.
///
/// Asserted end-to-end: an entry registered under the path
/// `positional_path` computes for a node *after* a packed run must be
/// found and applied by the walk, which reaches that node only through
/// `child_path` plus its own ordinal counter.
#[test]
fn the_forward_and_backward_ordinal_walks_agree_across_a_packed_run() {
    let (mut app, _run, tail, _a, _b) = packed_run_with_tail_fixture();

    let tail_path = app.positional_path(tail);
    assert_eq!(tail_path, "/2");
    app.overrides.activate(
        OverrideOrigin::Path {
            path: tail_path.clone(),
        },
        Some("test.Outer".to_string()),
    );
    app.render_overrides(app.first_node);

    assert_eq!(
        app.provenance
            .get(app.tree[tail].rendered_as)
            .map(|(target, _)| target.clone()),
        Some(Some(Some("test.Outer".to_string()))),
        "the walk must have reached {tail_path} and applied its entry — \
         if it did not, the forward counter disagrees with \
         `positional_path`"
    );
}

/// Spec 0143 regression: overriding a
/// varint-wire-framed field to an incompatible primitive type (any
/// `Kind::Double`/`Float`/`Fixed32`/`Fixed64`/`String`/`Bytes`/
/// `Message` target hits `prototext-core`'s `VarintKind::Mismatch`
/// catch-all, which writes the numeric field key and a `TYPE_MISMATCH`
/// flag, never the synthetic field-name placeholder) must not corrupt
/// the `TYPE_MISMATCH` annotation by splicing the field name into it: a
/// naive `.replacen('_', ..)` produces `TYPEtype_idMISMATCH`.
#[test]
fn splice_override_on_a_varint_mismatch_does_not_corrupt_type_mismatch_annotation() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto3_fds_in(
        "varint_mismatch",
        "varint_mismatch.proto",
        vec![message(
            "IntHolder",
            vec![field("type_id", 2, Label::Optional, Type::Int32)],
        )],
    );

    // IntHolder { type_id: 5 } — field 2, varint wire type.
    let mut app = fixture_under(
        "varint-mismatch-override",
        &fds,
        "varint_mismatch.IntHolder",
        &[0x10u8, 0x05],
    );

    let idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 2)
        .expect("must find field 2");

    app.splice_override(idx, Some("double".to_string()), None)
        .expect("override onto an incompatible primitive type must still succeed");

    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("TYPE_MISMATCH")),
        "mismatch must surface as an intact inline annotation: {:?}",
        app.document_lines()
    );
    assert!(
        !app.document_lines().iter().any(|l| l.contains("type_id")),
        "the field name must never be spliced into a mismatched line: {:?}",
        app.document_lines()
    );
}

/// Spec 0222, test-plan item 6 — the direct statement of G2: a commit
/// writes text only inside the subtree it re-rendered.
///
/// This is what deleting the whole-document `lines` was for. While the
/// text was one `Vec<String>`, a splice that changed a subtree's height
/// had to move every line after it, so the cost of a keystroke was set
/// by how much document followed the cursor rather than by what
/// changed. With the text in the nodes there is nothing after the
/// subtree to move, and the way to say so is that nothing outside it
/// was written.
///
/// The subtree is taken from the *arena*, not from the rendered tree:
/// a splice vacates the slots the superseded interpretation occupied,
/// and those stop being rendered children of anything while remaining
/// squarely inside the region that was re-rendered.
#[test]
fn a_commit_touches_only_the_spliced_subtree() {
    let (mut app, inner_idx, _) = type_as_fixture();

    let inside: Vec<bool> = {
        let parent = app.arena.parent();
        (0..app.node_text.len())
            .map(|s| {
                let mut cur = s;
                loop {
                    if cur == inner_idx {
                        return true;
                    }
                    let p = parent[cur] as usize;
                    if p == cur {
                        return false;
                    }
                    cur = p;
                }
            })
            .collect()
    };

    // `to_vec`, not `clone`: spec 0274 S8 made the field an `Arc`, and
    // cloning that would take a second owner of the very vector the
    // splice is about to write, rather than a snapshot to compare it
    // against.
    let before = app.node_text.to_vec();
    app.splice_override(inner_idx, Some("string".to_string()), None)
        .expect("overriding a submessage field as string must succeed");

    let touched: Vec<usize> = (0..before.len())
        .filter(|&s| before[s] != app.node_text[s])
        .collect();
    assert!(
        !touched.is_empty(),
        "the splice must actually change the rendering, or this proves nothing"
    );
    assert!(
        touched.len() < before.len(),
        "the fixture must have slots outside the spliced subtree, or this \
         proves nothing"
    );
    for &s in &touched {
        assert!(
            inside[s],
            "slot {s} was rewritten by a splice of slot {inner_idx}, which \
             does not contain it: {:?} -> {:?}",
            before[s], app.node_text[s]
        );
    }
}

/// Deactivating an override that turns a wide single overridden line
/// back into a narrower multi-line message must re-clamp `pan_offset`
/// to the new content — regression for a bug where panning all the way
/// right while a submessage field is overridden to a wide one-line
/// rendering, then deactivating the override (reverting to the real,
/// narrower multi-line message), left every visible row shorter than
/// `pan_offset`, so the main pane rendered blank — recoverable only by
/// panning right again. The re-clamp belongs on `finalize_override_
/// batch`, the chokepoint every splice passes through.
///
/// The wide line comes from `string`, whose escaped-bytes rendering of
/// the submessage's payload is longer than any line of the message
/// itself. It used to come from `int32` and its `TYPE_MISMATCH`
/// annotation, which spec 0219 replaced with a multi-line packed run.
#[test]
fn deactivating_override_reclamps_pan_offset_to_the_shrunk_content() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.main_area = Rect::new(0, 0, 10, 5);

    let widest_before = app.max_visible_line_len();
    app.splice_override(inner_idx, Some("string".to_string()), None)
        .expect("overriding a submessage field as string must succeed");
    assert!(
        app.max_visible_line_len() > widest_before,
        "fixture must actually widen the document: {:?}",
        app.document_lines()
    );

    for _ in 0..50 {
        app.pan_right();
    }
    assert!(
        app.pan_offset > 0,
        "fixture must actually exercise panning while overridden"
    );

    // Reverting an active override falls back to the field's *natural*
    // type (`resettle_node`'s `natural_type(idx)`), not a raw `None`
    // retype (spec 0135 G1) — `None` is a distinct, explicit "show
    // raw" choice a user can select in the override pane, not what
    // happens when an override is simply removed.
    app.splice_override(inner_idx, Some("test.Inner".to_string()), None)
        .unwrap();

    let usable_width = app.main_area.width as usize - 1;
    let max_len = app.max_visible_line_len();
    let expected_max_pan = max_len.saturating_sub(usable_width);
    assert!(
        app.pan_offset <= expected_max_pan,
        "pan_offset ({}) must be re-clamped to the reverted content's width \
         ({expected_max_pan}), or the main pane renders blank",
        app.pan_offset,
    );
}

/// Deactivating an override must recompute `max_visible_line_len`'s
/// clamping bound against the window the *next* render will actually
/// show — not a stale `scroll.index` left over from before the
/// splice (2026-07-24 follow-up feedback, after the blank-pane fix
/// above): the cursor sits on a *sibling* field after `inner`, whose
/// row shifts down several lines once `inner`'s body re-expands from
/// the override's 1-line collapse back to its real 4-line shape, so
/// `scroll.index` must advance to keep that sibling in view — same
/// as `render()`'s own `clamp_scroll_to_visible` would do. Using the
/// stale, pre-splice `scroll.index` instead leaves the window
/// pointing at the wrong rows, missing the wide field row that
/// scrolled into view alongside the cursor, and under-estimates the
/// true visible width — over-clamping `pan_offset` too far left.
#[test]
fn deactivating_override_recomputes_the_pan_bound_against_the_post_splice_scroll_window() {
    use prost_types::field_descriptor_proto::{Label, Type};

    // The long field name is the fixture's whole point: once `inner`
    // reverts to its natural 4-line shape, this field's row must be
    // the widest currently-visible row — but only reachable by
    // scrolling down to follow the cursor's (shifted) footer row.
    const WIDE_FIELD_NAME: &str =
        "a_field_with_a_very_long_name_so_its_own_rendered_line_is_the_widest_visible_row";
    let scalar = |name: &str, number: i32| field(name, number, Label::Optional, Type::Int32);
    let fds = proto3_fds(
        "stale_scroll.proto",
        vec![
            message(
                "Outer",
                vec![
                    field_of("inner", 1, Label::Optional, Type::Message, ".test.Inner"),
                    scalar("pad_a", 2),
                    scalar("pad_b", 3),
                ],
            ),
            message("Inner", vec![scalar("id", 1), scalar(WIDE_FIELD_NAME, 2)]),
        ],
    );

    // Outer { inner: Inner { id: 5, <wide_field>: 7 }, pad_a: 1, pad_b: 2 }.
    let mut app = fixture_under(
        "stale-scroll-override",
        &fds,
        "test.Outer",
        &[
            0x0Au8, 0x04, 0x08, 0x05, 0x10, 0x07, // inner { id: 5, wide_field: 7 }
            0x10, 0x01, // pad_a: 1
            0x18, 0x02, // pad_b: 2
        ],
    );
    app.main_area = Rect::new(0, 0, 10, 3);

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the Inner submessage");
    let pad_a_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 2 && n.span.level == 1)
        .expect("tree must contain the pad_a sibling field");

    // Rest the cursor on the `pad_a` sibling field, as a normal render
    // pass would have already scrolled to keep it in view — seeds
    // `scroll.index`/`last_cursor_row` exactly as they'd be just
    // before the user deactivates the override.
    app.cursor = pad_a_idx;

    app.splice_override(inner_idx, Some("int32".to_string()), None)
        .expect("overriding a submessage field as int32 must still succeed (as a type mismatch)");

    let wide_field_row = app
        .document_lines()
        .iter()
        .position(|l| l.contains(WIDE_FIELD_NAME));
    assert!(
        wide_field_row.is_none(),
        "wide field must be hidden while inner is overridden as int32: {:?}",
        app.document_lines()
    );

    // Reverting an active override falls back to the field's *natural*
    // type (`resettle_node`'s `natural_type(idx)`), not a raw `None`
    // retype (spec 0135 G1). `inner` re-expands from its 1-line
    // collapse back to its real 4-line shape, pushing `pad_a` (the
    // cursor's node) several rows further down — `scroll.index` must
    // advance to keep it in view.
    app.splice_override(inner_idx, Some("test.Inner".to_string()), None)
        .unwrap();

    let wide_field_row = app
        .document_lines()
        .iter()
        .position(|l| l.contains(WIDE_FIELD_NAME))
        .expect("revert must restore the wide field");
    let wide_field_len = app
        .row_content(app.committed_row(wide_field_row).unwrap())
        .chars()
        .count();

    let max_len = app.max_visible_line_len();
    assert!(
        max_len >= wide_field_len,
        "max_visible_line_len ({max_len}) must reflect the post-splice scroll window \
         (which includes the {wide_field_len}-char-wide field row scrolled into view \
         alongside the cursor's shifted sibling field), not a stale pre-splice window"
    );
}

/// Overriding a group field to a resolvable type must keep the
/// `group;` prefix in the header (spec 0122 Test Plan item 2, 1st
/// bullet).
#[test]
fn splice_override_on_a_group_field_keeps_the_group_prefix() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), None)
        .unwrap();
    let header = &app.document_lines()[app.absolute_start(grp_idx)];
    assert!(
        header.contains("#@ group; NewGroup = 5"),
        "expected group; NewGroup = 5 in header, got: {header:?}"
    );
}

/// The header-line patch (spec 0122 §2) usually changes the header's
/// byte length (e.g. bare `#@ group` growing into `#@ group; NewGroup
/// = 5`) — the per-line color buckets must stay aligned with
/// `self.document_lines()`' actual text for every line *after* the header, not
/// just the header itself (2026-07-15 regression: colors drifted
/// starting right after the first patched `#@ group;` header,
/// because `hints_by_line` was bucketing hints computed against the
/// *cached* (unpatched) header length using the *patched* line
/// array's lengths).
///
/// Spec 0187 S3 removes that failure mode by construction — highlighting
/// is now parsed from the very strings about to be drawn, so there is no
/// second, staler copy of the text to disagree with. The property is
/// still worth pinning, now through the window path it actually travels.
#[test]
fn splice_override_keeps_colors_aligned_after_a_header_length_change() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), None)
        .unwrap();
    let value_idx = app
        .first_child(grp_idx)
        .expect("NewGroup has at least one child");
    let line_idx = app.absolute_start(value_idx);
    let line = app.document_lines()[line_idx].clone();
    let value_pos = line
        .find('5')
        .expect("value line must contain the scalar 5");

    let rows = app.visible_row_count();
    let window: Vec<DisplayRow> = app
        .visible_window(0, rows)
        .into_iter()
        .map(|(l, pos)| app.committed_row_at(l, pos))
        .collect();
    let window_index = app
        .visible_row_of_line(line_idx)
        .expect("the value line is visible — nothing is folded in this fixture");
    app.refresh_window_styles(&window);
    let styles = &app.window_styles[window_index];
    assert!(
        styles
            .iter()
            .any(|(range, role)| *role == SyntaxRole::Number && range.contains(&value_pos)),
        "expected a Number-colored span covering the '5' in {line:?}, got {styles:?}"
    );
}

/// A group field carrying a `tag_ohb` anomaly modifier keeps that
/// modifier verbatim after being overridden to a different type (spec
/// 0122 Test Plan item 2, 3rd bullet).
#[test]
fn splice_override_on_a_group_field_keeps_the_tag_ohb_modifier() {
    let (mut app, grp_idx) = group_type_fixture_with_tag_ohb();
    let header_before = app.document_lines()[app.absolute_start(grp_idx)].clone();
    assert!(
        header_before.contains("tag_ohb: 1"),
        "fixture must exercise the anomaly modifier, got: {header_before:?}"
    );
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), None)
        .unwrap();
    let header = &app.document_lines()[app.absolute_start(grp_idx)];
    assert!(
        header.contains("#@ group; NewGroup = 5; tag_ohb: 1"),
        "expected group; NewGroup = 5; tag_ohb: 1 in header, got: {header:?}"
    );
}

/// Overriding a `WT_LEN` (non-group) field to a resolvable type must
/// NOT show a `group;` prefix (spec 0122 Test Plan item 2, 2nd
/// bullet).
#[test]
fn splice_override_on_a_wt_len_field_has_no_group_prefix() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splice_override(inner_idx, Some("test.Outer".to_string()), None)
        .unwrap();
    let header = &app.document_lines()[app.absolute_start(inner_idx)];
    assert!(
        header.contains("#@ Outer = 1"),
        "expected Outer = 1 in header, got: {header:?}"
    );
    assert!(
        !header.contains("group;"),
        "WT_LEN field override must not show group;: {header:?}"
    );
}

/// A nested (non-root) field's header line must keep its leading
/// indentation after `splice_override` — the spec 0122 header-patching
/// rewrite of `new_lines[0]` must not drop the indentation the
/// synthetic wrapper's own render already computed via
/// `initial_level` (2026-07-15 regression: header lines of overridden
/// nested nodes lost their indentation while sibling/interior lines
/// stayed correctly indented).
#[test]
fn splice_override_preserves_the_header_line_indentation() {
    let (mut app, inner_idx, _) = type_as_fixture();
    let start = app.absolute_start(inner_idx);
    let indent_before =
        app.document_lines()[start].len() - app.document_lines()[start].trim_start().len();
    app.splice_override(inner_idx, Some("test.Outer".to_string()), None)
        .unwrap();
    let header = &app.document_lines()[app.absolute_start(inner_idx)];
    let indent_after = header.len() - header.trim_start().len();
    assert!(
        indent_after > 0,
        "fixture must exercise a nested (indented) field"
    );
    assert_eq!(
        indent_after, indent_before,
        "header line lost its indentation after splice_override: {header:?}"
    );
}

/// Reverting a group field's override (`target: None`) must restore
/// bare `#@ group` — the synthetic wrapper's `"message"` placeholder
/// must not leak into the header (spec 0122 Test Plan item 2, 4th
/// bullet; user-approved fix, 2026-07-15).
#[test]
fn splice_override_reverting_a_group_field_restores_bare_group() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), None)
        .unwrap();
    app.splice_override(grp_idx, None, None).unwrap();
    let header = &app.document_lines()[app.absolute_start(grp_idx)];
    assert!(
        header.contains("#@ group"),
        "expected bare group in header, got: {header:?}"
    );
    assert!(
        !header.contains("message"),
        "reverted group header must not leak the synthetic \"message\" placeholder: {header:?}"
    );
}

/// The document root is field number 1 of the virtual encompassing
/// message — a `splice_override`-driven re-render of the root (any
/// retype, not just the initial `decode()` paint) must keep showing
/// its field number in the header line, same as
/// `decode_shows_the_root_field_number_in_the_header_line`
/// (`decode.rs`) covers for the initial paint.
#[test]
fn splice_override_shows_the_root_field_number_in_the_header_line() {
    let (mut app, _, _) = type_as_fixture();
    app.splice_override(app.first_node, Some("test.Outer".to_string()), None)
        .unwrap();
    assert!(
        app.document_lines()[0].starts_with("1 "),
        "root header line must show the root field number: {:?}",
        app.document_lines()[0]
    );
}

/// Retyping the document root *raw* (no schema, `target: None`) must
/// also keep showing its field number in the header line — the root
/// is not special-cased regardless of `target`.
#[test]
fn splice_override_raw_root_shows_the_field_number_in_the_header_line() {
    let (mut app, _, _) = type_as_fixture();
    app.splice_override(app.first_node, None, None).unwrap();
    assert!(
        app.document_lines()[0].starts_with("1 "),
        "raw root header line must show the field number: {:?}",
        app.document_lines()[0]
    );
}

/// Reproduces interactive-testing feedback (2026-07-14, post-D34): a
/// root node retyped raw (`None`) then retyped back to a real schema
/// must still expand its `Any` descendants — a bare re-splice of the
/// root shouldn't lose `Any` expansion the *initial* `render_overrides`
/// pass (spec 0120) got right. Fixture mirrors `decode.rs`'s own
/// `decode_leaves_any_fields_unexpanded_with_real_type_url_and_value_spans`.
#[test]
fn splice_override_reactivating_root_type_still_expands_any_fields() {
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{FileDescriptorProto, FileDescriptorSet};

    let acme_file = FileDescriptorProto {
        name: Some("acme.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![
            message(
                "Payload",
                vec![field("label", 1, Label::Optional, Type::String)],
            ),
            wrapper_message("Container", "payload", ".google.protobuf.Any"),
        ],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_proto_file(), acme_file],
    };

    // Container { payload: Any { type_url:
    // "type.googleapis.com/acme.Payload", value: Payload { label:
    // "hello" } } }.
    let blob = wrap_len_field_1(any_body(
        "type.googleapis.com/acme.Payload",
        b"\x0a\x05hello",
    ));
    let mut app = fixture_under("splice-any-reactivate", &fds, "acme.Container", &blob);

    // 1) retype the root raw (no schema) — mirrors the interactive
    //    "override / with raw/no-type" step, driven through the same
    //    `overrides.activate` + `render_overrides` path the `Enter`
    //    key handler uses in the override pane (not a bare
    //    `splice_override` call, which bypasses the recursive pass
    //    entirely and would miss this bug).
    let root_origin = override_pane::OverrideOrigin::Path {
        path: "/".to_string(),
    };
    app.overrides.activate(root_origin.clone(), None);
    app.render_overrides(app.first_node);
    // 2) retype the root back to the real schema — mirrors
    //    reactivating `acme.Container` in the management pane.
    app.overrides
        .activate(root_origin, Some("acme.Container".to_string()));
    app.render_overrides(app.first_node);

    assert!(
        has_node_with_type(&app, "acme.Payload"),
        "Any field must still expand after retyping the root away and \
         back: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hello")),
        "expanded Any payload's own field must appear in the \
         re-spliced text: {:?}",
        app.document_lines()
    );
}

/// Regression test (spec 0120 §G2, post-bugfix): a MessageSet's
/// group-wire "Item" entries (field 1, `WT_START_GROUP`) must be
/// decomposed via the two-tier auto-expansion (tier 1: synthetic
/// `protolens_internal.MessageSetItem`; tier 2: the specific
/// extension type resolved from `type_id`) through `render_overrides`
/// — not corrupted into a flat raw scalar, which is what happened
/// before the fix (`render_overrides`'s `is_message` recursion gate
/// unconditionally spliced the group node with no matching tier,
/// poisoning the render via `extract::message_payload_range`'s
/// documented "leaves the trailing END_GROUP tag" behavior for
/// `WT_START_GROUP`). Also asserts both tiers land as real,
/// persisted, active `OverrideEntry` rows (spec 0120 redesign: no
/// longer a silent dynamic fallback).
#[test]
fn message_set_group_items_auto_expand_through_render_overrides() {
    let app = message_set_fixture();

    assert!(
        has_node_with_type(&app, "ms_test.ExtPayload"),
        "MessageSet Item's message must auto-expand to the resolved \
         extension type: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "expanded MessageSet extension payload's own field must appear \
         in the spliced text: {:?}",
        app.document_lines()
    );

    let item_idx = node_with_type(&app, decode::MESSAGE_SET_ITEM_FQDN)
        .expect("Item group must be spliced to the synthetic MessageSetItem type");
    let item_path = app.positional_path(item_idx);
    assert!(
        app.overrides.entries().iter().any(|e| {
            e.active
                && matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path)
                && e.r#type.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN)
        }),
        "tier-1 auto-expansion must be a real, persisted, active \
         override entry: {:#?}",
        app.overrides.entries()
    );
    assert!(
        app.overrides.entries().iter().any(|e| {
            matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path)
                && e.name.as_deref() == Some("Item")
        }),
        "tier-1 auto-expansion must be seeded with the display name \
         \"Item\" (mirroring prototext-core's native MessageSet \
         rendering), not the bare field number: {:#?}",
        app.overrides.entries()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.trim_start().starts_with("Item {")),
        "the Item wrapper must render under the name \"Item\", not \
         its field number: {:?}",
        app.document_lines()
    );

    let message_idx = node_with_type(&app, "ms_test.ExtPayload")
        .expect("message field must resolve to ExtPayload");
    let message_path = app.positional_path(message_idx);
    assert!(
        app.overrides.entries().iter().any(|e| {
            e.active
                && matches!(&e.origin, OverrideOrigin::Path { path } if *path == message_path)
                && e.r#type.as_deref() == Some("ms_test.ExtPayload")
        }),
        "tier-2 auto-expansion must be a real, persisted, active \
         override entry: {:#?}",
        app.overrides.entries()
    );
}

/// Spec 0183 S1: auto-expansion must reach an `Any` buried under plain
/// message ancestors that carry no override of their own. Those
/// ancestors are exactly what the old `is_message` gate descended
/// through for free and what the seed scan's upward marking walk now
/// has to account for — a fixture with the `Any` directly under the
/// root would pass with that walk missing entirely.
#[test]
fn a_deeply_nested_any_is_auto_expanded() {
    let app = nested_any_fixture();

    assert!(
        has_node_with_type(&app, "acme.Payload"),
        "an Any three message levels down must still auto-expand: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hello")),
        "the expanded Any payload's own field must appear in the \
         rendered text: {:?}",
        app.document_lines()
    );

    // The expansion must come from the auto-override mechanism
    // (`auto_expand_type` seeding a real entry, spec 0120), not from
    // `decode`'s own paint — this is what makes the fixture a test of
    // the walk at all. `decode` deliberately leaves `Any` unexpanded
    // (see `decode_leaves_any_fields_unexpanded_with_real_type_url_
    // and_value_spans`), so a missing entry here would mean the
    // fixture is proving nothing.
    let value_idx =
        node_with_type(&app, "acme.Payload").expect("the Any's value must resolve to acme.Payload");
    let value_path = app.positional_path(value_idx);
    assert!(
        app.overrides.entries().iter().any(|e| {
            e.active
                && e.auto
                && matches!(&e.origin, OverrideOrigin::Path { path } if *path == value_path)
                && e.r#type.as_deref() == Some("acme.Payload")
        }),
        "the expansion must be a real, persisted, active *auto* \
         override entry: {:#?}",
        app.overrides.entries()
    );
}

/// Spec 0183 S1, the half that breaks silently: MessageSet tier 1 (the
/// `Item` group wrapper) is deliberately not an
/// `is_auto_expand_candidate`, on the recorded grounds that the
/// `is_message` disjunct reaches it. Both tiers must survive that
/// disjunct's removal, and must do so from under plain ancestors.
#[test]
fn a_deeply_nested_message_set_is_auto_expanded_at_both_tiers() {
    let app = nested_message_set_fixture();

    assert!(
        has_node_with_type(&app, crate::decode::MESSAGE_SET_ITEM_FQDN),
        "tier 1: the Item group wrapper must be retyped to the \
         synthetic Item shape: {:#?}",
        app.document_lines()
    );
    assert!(
        has_node_with_type(&app, "ms_test.ExtPayload"),
        "tier 2: the Item's message must resolve to the extension \
         type: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "the expanded extension payload's own field must appear in the \
         rendered text: {:?}",
        app.document_lines()
    );

    // Both tiers must be real auto-override entries, for the same
    // reason as in `a_deeply_nested_any_is_auto_expanded`.
    for (fqdn, tier) in [
        (crate::decode::MESSAGE_SET_ITEM_FQDN, "tier 1"),
        ("ms_test.ExtPayload", "tier 2"),
    ] {
        let idx = node_with_type(&app, fqdn).expect("the tier's node must exist");
        let path = app.positional_path(idx);
        assert!(
            app.overrides.entries().iter().any(|e| {
                e.active
                    && e.auto
                    && matches!(&e.origin, OverrideOrigin::Path { path: p } if *p == path)
                    && e.r#type.as_deref() == Some(fqdn)
            }),
            "{tier} must be a real, persisted, active *auto* override \
             entry: {:#?}",
            app.overrides.entries()
        );
    }
}

/// Regression test (2026-07-18 feedback item 4): the internal,
/// globally-shared `decode::MESSAGE_SET_ITEM_FQDN` (`protolens_internal
/// .Item`) must never leak into the two places a tier-1 Item node's
/// type is shown to the user — the status line and the manage pane —
/// both must instead show the friendly, MessageSet-specific FQDN
/// (`ms_test.TestMessageSet.Item` for this fixture's MessageSet).
#[test]
fn message_set_item_status_and_manage_labels_show_the_friendly_fqdn_not_the_internal_one() {
    let app = message_set_fixture();

    let item_idx = node_with_type(&app, decode::MESSAGE_SET_ITEM_FQDN)
        .expect("Item group must be spliced to the synthetic MessageSetItem type");

    let (status_label, _tag) = app
        .status_type_label(item_idx)
        .expect("Item node must have a status-line type label");
    assert!(
        status_label.contains("ms_test.TestMessageSet.Item"),
        "status line must show the friendly MessageSet-specific FQDN, \
         not the internal one: {status_label:?}"
    );
    assert!(
        !status_label.contains("protolens_internal"),
        "status line must never leak the internal namespace: \
         {status_label:?}"
    );

    let item_path = app.positional_path(item_idx);
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == item_path))
        .expect("tier-1 entry must exist");
    let manage_line = app.manage_type_line(entry_idx);
    assert!(
        manage_line.contains("ms_test.TestMessageSet.Item"),
        "manage pane must show the friendly MessageSet-specific FQDN, \
         not the internal one: {manage_line:?}"
    );
    assert!(
        !manage_line.contains("protolens_internal"),
        "manage pane must never leak the internal namespace: \
         {manage_line:?}"
    );
}

/// Spec 0185 G1/Q3, superseding the spec 0132 §G3 feedback fix
/// (2026-07-15): opening the override pane on an ancestor of a
/// MessageSet's `Item` group used to live-preview that ancestor via a
/// bare `splice_override`, which rebuilt the subtree with no
/// per-descendant overrides applied and so discarded every descendant's
/// tier-1/tier-2 auto-expansion — `Esc` then had to re-run the whole
/// `render_overrides` recursion to put it back. An overlay never
/// touches the committed tree, so the expansion is never lost and there
/// is nothing to restore.
#[test]
fn the_override_preview_leaves_nested_message_set_auto_expansion_alone() {
    let mut app = message_set_fixture();
    let extensions_idx = node_with_type(&app, "ms_test.TestMessageSet")
        .expect("extensions field must resolve to TestMessageSet");
    app.cursor = extensions_idx;
    let lines = app.document_lines().clone();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(extensions_idx));
    assert!(app.preview_overlay.is_some(), "the pane must live-preview");
    assert_eq!(
        app.document_lines(),
        lines,
        "the preview must leave the committed document alone"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(
        app.document_lines(),
        lines,
        "...and cancelling must re-render nothing"
    );
    assert!(
        has_node_with_type(&app, "ms_test.ExtPayload"),
        "the nested MessageSet auto-expansion must have survived: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "expanded MessageSet extension payload's own field must still be \
         in the rendered text: {:?}",
        app.document_lines()
    );
}

/// Spec 0117 §1 (amended): when neither `--type` nor inference resolves
/// a root type, `App::new` seeds no override at all — the collection
/// starts genuinely empty, not with a `path: "/"` entry typed `None`.
/// The root must still render raw with no panic, and a later real
/// `type-as` must still take effect (the pre-marked `rendered_as` must
/// not wrongly claim "already settled").
#[test]
fn no_resolved_root_type_seeds_no_override_and_still_renders_raw() {
    use crate::decode::{decode, DescriptorContext, RootType};

    let mut ctx = DescriptorContext::empty_for_test();
    // A single varint field (tag 0x08, value 5) — no --type, and this
    // context has no hopcroft.rkyv, so autoinference is unavailable.
    let blob = [0x08u8, 0x05];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Infer, 2).unwrap();
    assert_eq!(decoded.root_type, "<raw / no type>");

    let app = app_named(decoded, ctx, "test.pb");

    assert!(
        app.overrides.entries().is_empty(),
        "no root type resolved: the override collection must start empty, \
         got {:#?}",
        app.overrides.entries()
    );
}

/// Spec 0249 S1/S5, test-plan item 2: a splice given a row budget stops
/// at a node boundary, and every node it stopped at is auto-folded — so
/// the truncation reads as a collapsed subtree rather than as an empty
/// one.
///
/// Budget 2 on a wrapper holding three `Item`s: the wrapper's own header
/// is row 1, and the first `Item`'s header is row 2, at which point the
/// budget is spent and no `Item` gets a body.
#[test]
fn a_row_budgeted_splice_folds_every_node_it_stopped_at() {
    let (mut app, items) = repeated_message_fixture();
    let root = app.first_node;

    let full = app.document_lines();
    assert!(
        full.len() > 5,
        "fixture must have a body to leave out: {full:?}"
    );

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");

    assert_eq!(
        app.auto_folded.len(),
        items.len(),
        "each Item is one stop: {:?}",
        app.auto_folded
    );
    for i in &items {
        assert!(app.is_folded(*i), "Item {i} must be folded");
        assert_eq!(
            app.tree[*i].lines_total, 2,
            "a stopped node has a header and a footer and nothing between"
        );
        assert_eq!(app.tree[*i].lines_visible, 1, "and draws as one row");
    }
    assert!(
        app.folded.is_empty(),
        "an auto-fold is not a user fold (S3)"
    );

    // The rows the user sees: the wrapper's own two, plus one per Item.
    // `lines_total` stays at the unfolded height, as it does under any
    // fold — an auto-fold hides rows, it does not delete them, which is
    // what lets S8 expand one back without re-deriving the document.
    assert_eq!(
        app.tree[root].lines_visible as usize,
        2 + items.len(),
        "bounded document: {:?}",
        app.document_lines()
    );
    assert_eq!(
        app.tree[root].lines_total as usize,
        2 + 2 * items.len(),
        "and every stopped node still counts its own two"
    );

    // And the same splice unbounded restores every row, taking the
    // auto-folds back out with it.
    app.splice_override(root, Some("test.Outer".to_string()), None)
        .expect("an unbounded splice must succeed");
    assert!(
        app.auto_folded.is_empty(),
        "a rendered body owes no fold: {:?}",
        app.auto_folded
    );
    assert_eq!(app.document_lines(), full, "and shows what it did before");
}

/// Spec 0249 S6: the row budget's floor. A budget of 1 is spent on the
/// node's own header, so `descend` refuses the body and the node comes
/// back undescended — a two-line render drawn as one row.
///
/// The assertion that matters is not the count but the *content*: the
/// header the bounded render produces is the new one. An earlier draft
/// of S6 held that a node's header does not depend on its own type and
/// that an `--as <fqdn>` could therefore keep the old string verbatim
/// and skip the renderer. The `#@` annotation names the node's own
/// type, so it can't — and a revert changes the key as well, the field
/// name having come from the override too.
#[test]
fn a_budget_of_one_renders_exactly_the_header() {
    let (mut app, inner_idx, _) = type_as_fixture();

    let header = |app: &App| app.document_lines()[app.absolute_start(inner_idx)].clone();
    let before = header(&app);
    assert!(
        before.contains("inner {") && before.contains("Inner"),
        "fixture header: {before:?}"
    );

    app.splice_override(inner_idx, Some("test.Outer".to_string()), Some(1))
        .expect("a budget-1 splice must succeed");

    assert_eq!(
        app.tree[inner_idx].lines_total, 2,
        "a header and a footer and nothing between"
    );
    assert_eq!(app.tree[inner_idx].lines_visible, 1, "drawn as one row");
    assert!(
        app.is_folded(inner_idx),
        "the node the budget stopped at is folded: {:?}",
        app.auto_folded
    );
    let after = header(&app);
    assert!(
        after.contains("Outer"),
        "the header is the new one, not the old: {before:?} -> {after:?}"
    );

    // And reverting to raw rewrites the key, not just the annotation.
    app.splice_override(inner_idx, None, Some(1))
        .expect("a budget-1 revert must succeed");
    let raw = header(&app);
    assert!(
        !raw.contains("inner"),
        "a raw node shows its bare field number: {raw:?}"
    );
}

/// Spec 0249 S3 / open question 3: a bake does not clear a user fold,
/// and does not have to try not to.
///
/// A bake splices at a node whose body has not been rendered, so
/// `splice_override`'s scrub of `folded` walks a vacant subtree and
/// finds nothing to clear. That is the invariant this asserts first —
/// it is what makes the scrub safe on this path, and S11's bake carries
/// the matching `debug_assert!`.
#[test]
fn a_bake_keeps_the_user_folds_around_it() {
    let (mut app, items) = repeated_message_fixture();
    let root = app.first_node;

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");

    // The invariant: nothing is rendered under a stop, so nothing under
    // a stop can carry a fold the scrub would clear.
    for &stop in &app.auto_folded.clone() {
        let mut below = Vec::new();
        app.collect_descendants(stop, &mut below);
        assert!(
            below.is_empty(),
            "stop {stop} has rendered descendants: {below:?}"
        );
    }

    // The user folds one stop by hand. It is now in both sets.
    app.folded.insert(items[0]);

    // Bake a different one: unbounded, same interpretation.
    app.splice_override(items[1], Some("test.Item".to_string()), None)
        .expect("a bake must succeed");

    assert!(
        !app.auto_folded.contains(&items[1]),
        "a rendered body owes no fold"
    );
    assert!(app.tree[items[1]].lines_total > 2, "and shows its body");

    assert!(
        app.folded.contains(&items[0]),
        "the user's fold is untouched by a bake elsewhere"
    );
    assert!(app.is_folded(items[0]), "and still draws collapsed");

    // Baking the one the user folded clears only the auto-fold: the
    // gesture is what holds it collapsed from here on.
    app.splice_override(items[0], Some("test.Item".to_string()), None)
        .expect("a bake must succeed");
    assert!(!app.auto_folded.contains(&items[0]));
    assert!(app.folded.contains(&items[0]));
    assert!(app.is_folded(items[0]), "the user's fold still holds it");
    assert_eq!(app.tree[items[0]].lines_visible, 1);
}

// ── Spec 0253: an override keeps the field's cardinality ────────────────────

/// Spec 0253 G1, and the measured regression it was written for: before
/// it, a splice rooted at a repeated element rendered it behind a
/// synthetic *optional* field, so the header read `#@ Item = 1` where
/// the un-overridden node read `#@ repeated Item = 1`.
#[test]
fn an_override_keeps_the_repeated_qualifier() {
    let (mut app, items) = repeated_message_fixture();
    let before = app.document_lines()[app.absolute_start(items[0])].clone();
    assert!(
        before.contains("repeated Item"),
        "fixture must start out repeated: {before:?}"
    );

    app.splice_override(items[0], Some("test.Item".to_string()), None)
        .expect("retyping an element to its own type must succeed");

    assert_eq!(
        app.document_lines()[app.absolute_start(items[0])].clone(),
        before,
        "an override to the same type must not change the header at all"
    );
}

/// Spec 0253 S2: `required` is the other half of the rule, and it is
/// only reachable because `register_synthetic` declares the synthetic
/// wrapper file `proto2` — under proto3 the label would be rejected at
/// registration and the splice would fail outright.
#[test]
fn an_override_keeps_the_required_qualifier() {
    use prost_types::field_descriptor_proto::{Label, Type};

    let fds = proto2_fds(
        "test_required_cardinality.proto",
        vec![
            message(
                "Outer",
                vec![field_of(
                    "inner",
                    1,
                    Label::Required,
                    Type::Message,
                    ".test.Inner",
                )],
            ),
            message("Inner", vec![field("v", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // inner: field 1 (tag 0x0A, LEN) wrapping Inner { v: 5 }.
    let mut app = fixture_under(
        "required-cardinality",
        &fds,
        "test.Outer",
        &[0x0Au8, 0x02, 0x08, 0x05],
    );
    let inner = app
        .nth_child(app.first_node, 0)
        .expect("the root has the one `inner` field");

    let before = app.document_lines()[app.absolute_start(inner)].clone();
    assert!(
        before.contains("required Inner"),
        "fixture must start out required: {before:?}"
    );

    app.splice_override(inner, Some("test.Inner".to_string()), None)
        .expect("a required field must still be overridable");

    assert_eq!(
        app.document_lines()[app.absolute_start(inner)].clone(),
        before,
        "an override must keep the `required` qualifier too"
    );
}

/// Spec 0253: the common case does not gain a spurious qualifier —
/// `optional` is the default and prototext writes nothing for it.
#[test]
fn an_optional_field_gains_no_qualifier() {
    let (mut app, inner, _leaf) = type_as_fixture();
    app.splice_override(inner, Some("test.Inner".to_string()), None)
        .expect("retyping to its own type must succeed");

    let header = app.document_lines()[app.absolute_start(inner)].clone();
    assert!(
        !header.contains("repeated")
            && !header.contains("required")
            && !header.contains("optional"),
        "an optional field carries no qualifier at all: {header:?}"
    );
}

/// Spec 0253 N2, the "(if any)" case: nothing declares the field, so
/// there is no cardinality to inherit and the header says nothing.
/// Deliberately *not* guessed from how many siblings the document
/// happens to carry — that guess belongs to `export --descriptor`, which
/// has to name a label whatever happens.
#[test]
fn an_override_under_a_raw_parent_has_no_qualifier() {
    let (mut app, _inner, _known, unknown) = unknown_field_fixture();
    assert!(
        app.parent_field(unknown).is_none(),
        "field 9 must be the one nothing declares"
    );

    app.splice_override(unknown, Some("int32".to_string()), None)
        .expect("an unknown field is still overridable");

    let row = app.document_lines()[app.absolute_start(unknown)].clone();
    assert!(
        !row.contains("repeated") && !row.contains("required"),
        "an undeclared field inherits no qualifier: {row:?}"
    );
}

/// Spec 0253 S4 / spec 0219 S4: warming and the splice must hash to the
/// same wrapper name, or arrowing onto a candidate pays the registration
/// the warming pass exists to have already made. Observed by counting
/// the pool's files: a splice that finds its wrapper registers nothing.
#[test]
fn warming_registers_the_wrapper_the_splice_looks_up() {
    let (mut app, items) = repeated_message_fixture();
    app.override_target = Some(items[0]);
    app.override_candidates = vec![("test.Item".to_string(), None)];
    app.warm_visible_override_wrappers(0, 1);

    let warmed = app.ctx.pool().files().len();
    app.splice_override(items[0], Some("test.Item".to_string()), None)
        .expect("the splice must succeed");
    assert_eq!(
        app.ctx.pool().files().len(),
        warmed,
        "the splice must reuse the warmed wrapper, not register a second one"
    );
}

/// Spec 0256 S1 / test-plan item 1: the vacate loop hands the previous
/// interpretation's text to the idle loop rather than freeing it on the
/// keystroke.
///
/// Asserted on both halves at once, because the whole design rests on
/// them being one move: every vacated slot is `None`, and every box that
/// left one is in the vector. Anything that leaks a box would still pass
/// the first assertion alone.
#[test]
fn a_confirm_moves_the_old_text_aside_instead_of_dropping_it() {
    let (mut app, _) = repeated_message_fixture();
    app.bounded_confirms = true;
    let root = app.first_node;

    let mut descendants = Vec::new();
    app.collect_descendants(root, &mut descendants);
    let with_text = descendants
        .iter()
        .filter(|&&d| app.node_text[d].is_some())
        .count();
    assert!(with_text > 0, "the fixture must have text to move aside");
    let root_had_text = app.node_text[root].is_some();

    app.splice_override(root, Some("test.Outer".to_string()), None)
        .expect("the splice must succeed");

    assert_eq!(
        app.discarded_text.len(),
        with_text + usize::from(root_had_text),
        "every box the vacate loop took is in the vector"
    );
    // The invariant the move must not break, and the reason the boxes
    // go to a vector rather than being left in place: a vacant slot
    // (`lines_total == 0`) holds no text.
    for d in 0..app.tree.len() {
        assert!(
            app.tree[d].lines_total > 0 || app.node_text[d].is_none(),
            "slot {d} is vacant, so it must hold no text"
        );
    }
}

/// Spec 0256 S3 / test-plan item 2: the guard, and the only thing
/// standing between a scripted `export` and a vector that grows to hold
/// the whole previous document with nothing to empty it.
#[test]
fn a_headless_confirm_frees_inline() {
    let (mut app, _) = repeated_message_fixture();
    assert!(
        !app.bounded_confirms,
        "the default is the safe one: no event loop, no deferral"
    );
    let root = app.first_node;

    app.splice_override(root, Some("test.Outer".to_string()), None)
        .expect("the splice must succeed");

    assert!(
        app.discarded_text.is_empty(),
        "with nothing to drain it, the old text is freed where it is dropped"
    );
}

/// Spec 0256 S2 / test-plan item 3: the drain terminates, and says so.
///
/// `discard_step` is the whole of what `run_loop`'s idle arm calls, so
/// driving it directly is driving the loop. The step count is the
/// assertion that it is chunked rather than one unbounded free.
#[test]
fn the_idle_loop_empties_the_discarded_text() {
    let (mut app, _) = repeated_message_fixture();
    app.bounded_confirms = true;
    let root = app.first_node;
    app.splice_override(root, Some("test.Outer".to_string()), None)
        .expect("the splice must succeed");
    assert!(!app.discarded_text.is_empty());

    let mut steps = 0;
    while app.discard_step() {
        steps += 1;
        assert!(steps < 1000, "the drain must terminate");
    }
    assert!(app.discarded_text.is_empty());
    assert!(steps > 0, "there was something to free");
    assert!(
        !app.discard_step(),
        "and an empty vector is not progress, or the idle arm never sleeps"
    );
}

/// Spec 0256 S4 / test-plan item 4: fold flags are scrubbed by asking
/// the fold sets which of their entries sit under `idx`, not by asking
/// every slot of the document whether it is folded.
///
/// The mutation guard for S4 — dropping the `retain` entirely must fail
/// here — and equally the guard against scrubbing too much: a fold on a
/// sibling subtree is untouched, and `idx`'s own fold survives its own
/// retype (spec 0118 §7).
#[test]
fn a_fold_under_a_retyped_node_is_scrubbed() {
    let (mut app, items) = repeated_message_fixture();
    let under = app
        .nth_child(items[0], 0)
        .expect("Item has a field to fold");
    let sibling = items[1];

    // Seeded directly, because the four cases have to coexist and no
    // single gesture produces all of them. Both sets feed derived state
    // — the line counts and the `Unbaked` rung — and the splice under
    // test asserts both are exact, so they are brought back in line
    // here rather than left for the assertion to find.
    app.folded.insert(under);
    app.folded.insert(sibling);
    app.folded.insert(items[0]);
    app.auto_folded.insert(under);
    app.auto_folded.insert(sibling);
    app.refresh_line_counts(under);
    app.refresh_line_counts(items[0]);
    app.refresh_line_counts(sibling);
    app.rebuild_status();

    app.splice_override(items[0], Some("test.Item".to_string()), None)
        .expect("the splice must succeed");

    assert!(
        !app.folded.contains(&under) && !app.auto_folded.contains(&under),
        "a fold under the retyped node describes content that is gone"
    );
    assert!(
        app.folded.contains(&sibling) && app.auto_folded.contains(&sibling),
        "a fold on a sibling subtree is none of this splice's business"
    );
    assert!(
        app.folded.contains(&items[0]),
        "spec 0118 §7: a node keeps its own fold across its own retype"
    );
}
