// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::override_apply::{LinePatch, LinePatchTarget};
use super::super::*;
use super::support::*;
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};

/// Spec 0118 §4: `splice_override` regenerates a whole node (not just
/// its interior) into `self.lines`/`self.tree`, repeatable on the
/// same node (the design's key risk: post-order array contiguity does
/// not survive a *second* override of the same node, since the first
/// override's new nodes are appended at the array's end —
/// `splice_override` must never rely on it).
#[test]
fn apply_override_splices_tree_and_lines_repeatedly() {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let leaf_desc = DescriptorProto {
        name: Some("Leaf".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("val".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let node_desc = DescriptorProto {
        name: Some("Node".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("a".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Leaf".to_string()),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("b".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("inner".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Node".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_apply_override.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, node_desc, leaf_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let descriptor_path = std::env::temp_dir().join("protolens-tui-apply-override-descriptor.pb");
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Node payload: a = Leaf { val: 9 } (message, field 1), b = 42
    // (varint, field 2).
    let leaf_bytes = [0x08u8, 0x09];
    let node_payload = [0x0Au8, 0x02, leaf_bytes[0], leaf_bytes[1], 0x10, 0x2A];
    // Outer wraps Node as field 1 (LEN).
    let mut blob = vec![0x0Au8, node_payload.len() as u8];
    blob.extend_from_slice(&node_payload);

    let decoded = decode(&blob, &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );

    let node_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Node"))
        .expect("tree must contain the Node submessage");
    let node_level = app.tree[node_idx].span.level;

    // Fold the "a" child before overriding, to verify the stale-fold
    // scrubbing (`collect_descendants` cleanup).
    let a_idx_before = app.tree[node_idx]
        .first_child
        .expect("Node has at least one child");
    // Spec 0210 S2: through `refresh_line_counts`, since a fold now moves
    // the line counters the row walk reads.
    app.folded.insert(a_idx_before);
    app.refresh_line_counts(a_idx_before);

    let assert_children = |app: &App, tag: &str| {
        let mut children = Vec::new();
        let mut cur = app.tree[node_idx].first_child;
        while let Some(c) = cur {
            children.push(c);
            cur = app.tree[c].next_sibling;
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
            app.tree[children[0]].span.type_fqdn.as_deref(),
            Some("test.Leaf"),
            "{tag}: first child must resolve to test.Leaf"
        );
    };

    app.override_target = Some(node_idx);

    // 1) Re-typed as itself: idempotent structural round-trip.
    app.splice_override(node_idx, Some("test.Node".to_string()), false, None)
        .expect("re-typing as the same type must succeed");
    assert_children(&app, "re-typed as itself");
    assert_eq!(
        app.tree[node_idx].span.type_fqdn.as_deref(),
        Some("test.Node")
    );
    assert!(
        !app.folded.contains(&a_idx_before),
        "orphaned old child must be scrubbed from `folded`"
    );

    // 2) Raw override (no schema).
    app.splice_override(node_idx, None, false, None)
        .expect("raw override must succeed");
    assert_eq!(app.tree[node_idx].span.type_fqdn, None);

    // 3) Re-typed again, on top of two prior overrides — exercises
    // repeated overrides of the same node.
    app.splice_override(node_idx, Some("test.Node".to_string()), false, None)
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
    for line in 0..app.lines.len() {
        app.heat_cue_for(line);
    }

    // Spec 0210 S2: line ownership must stay fully consistent with the
    // doc chain. Every node reachable via `doc_next` from `first_node`
    // owns its own header line, and nothing else does — which is what
    // the `line_to_node` vector used to be asserted to be, now asked of
    // the counters instead.
    let mut owners: Vec<Option<u32>> = vec![None; app.lines.len()];
    let mut count = 0;
    let mut cur = Some(app.first_node);
    while let Some(c) = cur {
        let line = app.absolute_start(c);
        assert_eq!(
            app.line_pos(line).map(|p| (p.node, p.footer)),
            Some((c, false)),
            "line {line} must resolve to node {c}'s header"
        );
        owners[line] = Some(c as u32);
        count += 1;
        assert!(count <= app.tree.len(), "doc chain must not cycle");
        cur = app.tree[c].doc_next;
    }
    for (line, owner) in owners.iter().enumerate() {
        if owner.is_none() {
            assert!(
                app.line_pos(line).is_some_and(|p| p.footer),
                "line {line} is no node's header, so it must be a footer"
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
    use crate::decode::{decode, DescriptorContext, RootType};
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    let str_msg = DescriptorProto {
        name: Some("StrHolder".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("s".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let target_msg = DescriptorProto {
        name: Some("Target".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("incompat.proto".to_string()),
        package: Some("incompat".to_string()),
        message_type: vec![str_msg, target_msg],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-incompat-override-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // StrHolder { s: "hello" }
    let label = b"hello";
    let mut blob = vec![0x0Au8, label.len() as u8];
    blob.extend_from_slice(label);
    let decoded = decode(&blob, &mut ctx, RootType::Named("incompat.StrHolder"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    let s_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("must find field 1");
    assert!(
        app.can_override(s_idx),
        "a WT_LEN scalar must be overridable"
    );

    app.splice_override(s_idx, Some("incompat.Target".to_string()), false, None)
        .expect("override onto an incompatible type must still succeed");
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("INVALID") || l.contains("TYPE_MISMATCH")),
        "mismatch must surface as an inline annotation, not a panic: {:?}",
        app.lines
    );
}

/// Spec 0184 G1: a packed run's elements must not be separately
/// numbered children, because committing an override on the run
/// collapses them into one node (`splice_override`'s `siblings[0]`
/// merge) — which used to renumber every later sibling and silently
/// re-point any path recorded before the commit.
#[test]
fn overriding_a_packed_run_does_not_renumber_later_siblings() {
    let (mut app, elems, tail, _a, _b) = packed_run_with_tail_fixture();

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

    app.override_target = Some(elems[0]);
    app.splice_override(elems[0], None, false, None)
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

/// Spec 0184 test plan, "ordinal stability across override state": the
/// whole path map is identical before an override on a packed run,
/// while it is active, and after deactivating it. This is the property
/// the previous test checks pointwise, asserted over every live node
/// and across the full activate/deactivate cycle — the direction that
/// used to shift ordinals *back*.
#[test]
fn packed_run_ordinals_are_stable_across_the_override_lifecycle() {
    let (mut app, elems, tail, a, b) = packed_run_with_tail_fixture();

    let path_map = |app: &App| -> Vec<(usize, String)> {
        let root = app
            .tree
            .iter()
            .position(|n| n.parent.is_none())
            .expect("tree must have a root");
        let mut out = Vec::new();
        let mut cur = Some(root);
        while let Some(i) = cur {
            out.push((i, app.positional_path(i)));
            cur = app.tree[i].doc_next;
        }
        out
    };

    let baseline = path_map(&app);
    let watched: Vec<String> = [tail, a, b]
        .iter()
        .map(|&i| app.positional_path(i))
        .collect();

    let origin = OverrideOrigin::Path {
        path: app.positional_path(elems[0]),
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
    let (mut app, _elems, tail, _a, _b) = packed_run_with_tail_fixture();

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
        app.tree[tail]
            .rendered_as
            .as_ref()
            .map(|(target, _)| target.clone()),
        Some(Some(Some("test.Outer".to_string()))),
        "the walk must have reached {tail_path} and applied its entry — \
         if it did not, the forward counter disagrees with \
         `positional_path`"
    );
}

/// Spec 0143 regression (2026-07-18 feedback): overriding a
/// varint-wire-framed field to an incompatible primitive type (any
/// `Kind::Double`/`Float`/`Fixed32`/`Fixed64`/`String`/`Bytes`/
/// `Message` target hits `prototext-core`'s `VarintKind::Mismatch`
/// catch-all, which writes the numeric field key and a `TYPE_MISMATCH`
/// flag, never the synthetic field-name placeholder) must not corrupt
/// the `TYPE_MISMATCH` annotation by splicing the field name into it —
/// the naive `.replacen('_', ..)` this spec replaced used to produce
/// `TYPEtype_idMISMATCH`.
#[test]
fn splice_override_on_a_varint_mismatch_does_not_corrupt_type_mismatch_annotation() {
    use crate::decode::{decode, DescriptorContext, RootType};
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    let msg = DescriptorProto {
        name: Some("IntHolder".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("type_id".to_string()),
            number: Some(2),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("varint_mismatch.proto".to_string()),
        package: Some("varint_mismatch".to_string()),
        message_type: vec![msg],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-varint-mismatch-override-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // IntHolder { type_id: 5 } — field 2, varint wire type.
    let blob = vec![0x10u8, 0x05];
    let decoded = decode(
        &blob,
        &mut ctx,
        RootType::Named("varint_mismatch.IntHolder"),
        2,
    )
    .unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    let idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 2)
        .expect("must find field 2");

    app.splice_override(idx, Some("double".to_string()), false, None)
        .expect("override onto an incompatible primitive type must still succeed");

    assert!(
        app.lines.iter().any(|l| l.contains("TYPE_MISMATCH")),
        "mismatch must surface as an intact inline annotation: {:?}",
        app.lines
    );
    assert!(
        !app.lines.iter().any(|l| l.contains("type_id")),
        "the field name must never be spliced into a mismatched line: {:?}",
        app.lines
    );
}

/// Deactivating an override that turns a narrow overridden line back
/// into a wide multi-line message must re-clamp `pan_offset` to the
/// new content — regression for a bug where panning all the way right
/// while a submessage field is mis-overridden as `int32` (a genuine
/// `TYPE_MISMATCH`, whose annotation renders a wide single line), then
/// deactivating the override (reverting to the real, narrower
/// multi-line message), left every visible row shorter than
/// `pan_offset`, so the main pane rendered blank — recoverable only by
/// panning right again (2026-07-24 bug report). `rebuild_visible_rows`
/// (the chokepoint `splice_override` always calls) must re-clamp.
#[test]
fn deactivating_override_reclamps_pan_offset_to_the_shrunk_content() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.main_area = Rect::new(0, 0, 10, 5);

    app.splice_override(inner_idx, Some("int32".to_string()), false, None)
        .expect("overriding a submessage field as int32 must still succeed (as a type mismatch)");
    assert!(
        app.lines.iter().any(|l| l.contains("TYPE_MISMATCH")),
        "fixture must trigger a TYPE_MISMATCH annotation to get a wide line: {:?}",
        app.lines
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
    app.splice_override(inner_idx, Some("test.Inner".to_string()), false, None)
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
/// show — not a stale `scroll_offset` left over from before the
/// splice (2026-07-24 follow-up feedback, after the blank-pane fix
/// above): the cursor sits on a *sibling* field after `inner`, whose
/// row shifts down several lines once `inner`'s body re-expands from
/// the override's 1-line collapse back to its real 4-line shape, so
/// `scroll_offset` must advance to keep that sibling in view — same
/// as `render()`'s own `clamp_scroll_to_visible` would do. Using the
/// stale, pre-splice `scroll_offset` instead leaves the window
/// pointing at the wrong rows, missing the wide field row that
/// scrolled into view alongside the cursor, and under-estimates the
/// true visible width — over-clamping `pan_offset` too far left.
#[test]
fn deactivating_override_recomputes_the_pan_bound_against_the_post_splice_scroll_window() {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    // The long field name is the fixture's whole point: once `inner`
    // reverts to its natural 4-line shape, this field's row must be
    // the widest currently-visible row — but only reachable by
    // scrolling down to follow the cursor's (shifted) footer row.
    const WIDE_FIELD_NAME: &str =
        "a_field_with_a_very_long_name_so_its_own_rendered_line_is_the_widest_visible_row";
    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some(WIDE_FIELD_NAME.to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("inner".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Inner".to_string()),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("pad_a".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("pad_b".to_string()),
                number: Some(3),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("stale_scroll.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-stale-scroll-override-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner { id: 5, <wide_field>: 7 }, pad_a: 1, pad_b: 2 }.
    let blob = [
        0x0Au8, 0x04, 0x08, 0x05, 0x10, 0x07, // inner { id: 5, wide_field: 7 }
        0x10, 0x01, // pad_a: 1
        0x18, 0x02, // pad_b: 2
    ];
    let decoded = decode(&blob, &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    app.main_area = Rect::new(0, 0, 10, 3);

    let inner_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
        .expect("tree must contain the Inner submessage");
    let pad_a_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 2 && n.span.level == 1)
        .expect("tree must contain the pad_a sibling field");

    // Rest the cursor on the `pad_a` sibling field, as a normal render
    // pass would have already scrolled to keep it in view — seeds
    // `scroll_offset`/`last_cursor_row` exactly as they'd be just
    // before the user deactivates the override.
    app.cursor = pad_a_idx;

    app.splice_override(inner_idx, Some("int32".to_string()), false, None)
        .expect("overriding a submessage field as int32 must still succeed (as a type mismatch)");

    let wide_field_row = app.lines.iter().position(|l| l.contains(WIDE_FIELD_NAME));
    assert!(
        wide_field_row.is_none(),
        "wide field must be hidden while inner is overridden as int32: {:?}",
        app.lines
    );

    // Reverting an active override falls back to the field's *natural*
    // type (`resettle_node`'s `natural_type(idx)`), not a raw `None`
    // retype (spec 0135 G1). `inner` re-expands from its 1-line
    // collapse back to its real 4-line shape, pushing `pad_a` (the
    // cursor's node) several rows further down — `scroll_offset` must
    // advance to keep it in view.
    app.splice_override(inner_idx, Some("test.Inner".to_string()), false, None)
        .unwrap();

    let wide_field_row = app
        .lines
        .iter()
        .position(|l| l.contains(WIDE_FIELD_NAME))
        .expect("revert must restore the wide field");
    let wide_field_len = app
        .row_content(DisplayRow::Committed(wide_field_row))
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
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), false, None)
        .unwrap();
    let header = &app.lines[app.absolute_start(grp_idx)];
    assert!(
        header.contains("#@ group; NewGroup = 5"),
        "expected group; NewGroup = 5 in header, got: {header:?}"
    );
}

/// The header-line patch (spec 0122 §2) usually changes the header's
/// byte length (e.g. bare `#@ group` growing into `#@ group; NewGroup
/// = 5`) — the per-line color buckets must stay aligned with
/// `self.lines`' actual text for every line *after* the header, not
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
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), false, None)
        .unwrap();
    let value_idx = app.tree[grp_idx]
        .first_child
        .expect("NewGroup has at least one child");
    let line_idx = app.absolute_start(value_idx);
    let line = app.lines[line_idx].clone();
    let value_pos = line
        .find('5')
        .expect("value line must contain the scalar 5");

    let rows = app.visible_row_count();
    let window: Vec<DisplayRow> = app
        .visible_window(0, rows)
        .into_iter()
        .map(|(l, _)| DisplayRow::Committed(l))
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
    let header_before = app.lines[app.absolute_start(grp_idx)].clone();
    assert!(
        header_before.contains("tag_ohb: 1"),
        "fixture must exercise the anomaly modifier, got: {header_before:?}"
    );
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), false, None)
        .unwrap();
    let header = &app.lines[app.absolute_start(grp_idx)];
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
    app.splice_override(inner_idx, Some("test.Outer".to_string()), false, None)
        .unwrap();
    let header = &app.lines[app.absolute_start(inner_idx)];
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
    let indent_before = app.lines[start].len() - app.lines[start].trim_start().len();
    app.splice_override(inner_idx, Some("test.Outer".to_string()), false, None)
        .unwrap();
    let header = &app.lines[app.absolute_start(inner_idx)];
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
    app.splice_override(grp_idx, Some("test.NewGroup".to_string()), false, None)
        .unwrap();
    app.splice_override(grp_idx, None, false, None).unwrap();
    let header = &app.lines[app.absolute_start(grp_idx)];
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
    app.splice_override(app.first_node, Some("test.Outer".to_string()), false, None)
        .unwrap();
    assert!(
        app.lines[0].starts_with("1 "),
        "root header line must show the root field number: {:?}",
        app.lines[0]
    );
}

/// Retyping the document root *raw* (no schema, `target: None`) must
/// also keep showing its field number in the header line — the root
/// is not special-cased regardless of `target`.
#[test]
fn splice_override_raw_root_shows_the_field_number_in_the_header_line() {
    let (mut app, _, _) = type_as_fixture();
    app.splice_override(app.first_node, None, false, None)
        .unwrap();
    assert!(
        app.lines[0].starts_with("1 "),
        "raw root header line must show the field number: {:?}",
        app.lines[0]
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
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let any_msg = DescriptorProto {
        name: Some("Any".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("type_url".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("value".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Bytes as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let any_file = FileDescriptorProto {
        name: Some("google/protobuf/any.proto".to_string()),
        syntax: Some("proto3".to_string()),
        package: Some("google.protobuf".to_string()),
        message_type: vec![any_msg],
        ..Default::default()
    };

    let payload_msg = DescriptorProto {
        name: Some("Payload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("payload".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".google.protobuf.Any".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let acme_file = FileDescriptorProto {
        name: Some("acme.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![payload_msg, container_msg],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_file, acme_file],
    };

    let descriptor_path =
        std::env::temp_dir().join("protolens-tui-splice-any-reactivate-descriptor.pb");
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Container { payload: Any { type_url:
    // "type.googleapis.com/acme.Payload", value: Payload { label:
    // "hello" } } }.
    let label = b"hello";
    let mut payload_bytes = vec![0x0au8, label.len() as u8];
    payload_bytes.extend_from_slice(label);
    let type_url = b"type.googleapis.com/acme.Payload";
    let mut any_bytes = vec![0x0au8, type_url.len() as u8];
    any_bytes.extend_from_slice(type_url);
    any_bytes.push(0x12);
    any_bytes.push(payload_bytes.len() as u8);
    any_bytes.extend_from_slice(&payload_bytes);
    let mut blob = vec![0x0au8, any_bytes.len() as u8];
    blob.extend_from_slice(&any_bytes);

    let decoded = decode(&blob, &mut ctx, RootType::Named("acme.Container"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

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
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload")),
        "Any field must still expand after retyping the root away and \
         back: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hello")),
        "expanded Any payload's own field must appear in the \
         re-spliced text: {:?}",
        app.lines
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
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "MessageSet Item's message must auto-expand to the resolved \
         extension type: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "expanded MessageSet extension payload's own field must appear \
         in the spliced text: {:?}",
        app.lines
    );

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
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
        app.lines
            .iter()
            .any(|l| l.trim_start().starts_with("Item {")),
        "the Item wrapper must render under the name \"Item\", not \
         its field number: {:?}",
        app.lines
    );

    let message_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload"))
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
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload")),
        "an Any three message levels down must still auto-expand: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hello")),
        "the expanded Any payload's own field must appear in the \
         rendered text: {:?}",
        app.lines
    );

    // The expansion must come from the auto-override mechanism
    // (`auto_expand_type` seeding a real entry, spec 0120), not from
    // `decode`'s own paint — this is what makes the fixture a test of
    // the walk at all. `decode` deliberately leaves `Any` unexpanded
    // (see `decode_leaves_any_fields_unexpanded_with_real_type_url_
    // and_value_spans`), so a missing entry here would mean the
    // fixture is proving nothing.
    let value_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload"))
        .expect("the Any's value must resolve to acme.Payload");
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
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some(crate::decode::MESSAGE_SET_ITEM_FQDN)),
        "tier 1: the Item group wrapper must be retyped to the \
         synthetic Item shape: {:#?}",
        app.lines
    );
    assert!(
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "tier 2: the Item's message must resolve to the extension \
         type: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "the expanded extension payload's own field must appear in the \
         rendered text: {:?}",
        app.lines
    );

    // Both tiers must be real auto-override entries, for the same
    // reason as in `a_deeply_nested_any_is_auto_expanded`.
    for (fqdn, tier) in [
        (crate::decode::MESSAGE_SET_ITEM_FQDN, "tier 1"),
        ("ms_test.ExtPayload", "tier 2"),
    ] {
        let idx = app
            .tree
            .iter()
            .position(|n| n.span.type_fqdn.as_deref() == Some(fqdn))
            .expect("the tier's node must exist");
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

    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
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
    let extensions_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("ms_test.TestMessageSet"))
        .expect("extensions field must resolve to TestMessageSet");
    app.cursor = extensions_idx;
    let lines = app.lines.clone();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(extensions_idx));
    assert!(app.preview_overlay.is_some(), "the pane must live-preview");
    assert_eq!(
        app.lines, lines,
        "the preview must leave the committed document alone"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(app.lines, lines, "...and cancelling must re-render nothing");
    assert!(
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "the nested MessageSet auto-expansion must have survived: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "expanded MessageSet extension payload's own field must still be \
         in the rendered text: {:?}",
        app.lines
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
    let decoded = decode(&blob, &mut ctx, RootType::Infer, 2).unwrap();
    assert_eq!(decoded.root_type, "<raw / no type>");

    let app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );

    assert!(
        app.overrides.entries().is_empty(),
        "no root type resolved: the override collection must start empty, \
         got {:#?}",
        app.overrides.entries()
    );
}

// ── Spec 0156 G6c/G6d: `resolve_export_fields` ─────────────────────────────

fn field_by_number(
    fields: &[export_descriptor::ResolvedField],
    number: u64,
) -> &export_descriptor::ResolvedField {
    fields
        .iter()
        .find(|f| f.number == number)
        .unwrap_or_else(|| panic!("no resolved field for number {number}, got {fields:#?}"))
}

impl std::fmt::Debug for export_descriptor::ResolvedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedField")
            .field("number", &self.number)
            .field("name", &self.name)
            .field("label", &self.label)
            .field("type", &self.r#type)
            .field("type_name", &self.type_name)
            .finish()
    }
}

/// G6c tier 3: a child whose parent's schema resolves it to a
/// primitive type uses that type directly, no dependency.
#[test]
fn resolve_export_fields_tier3_resolves_a_schema_primitive() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let num = field_by_number(&fields, 1);
    assert_eq!(num.r#type, Type::Int32);
    assert!(num.type_name.is_none());
    assert!(num.referenced_file.is_none());
}

/// G6c tier 3 + G6d: a child resolving to a message in a *different*
/// file declares that file as a dependency; a second child resolving
/// to a message in the cursor's *own* file also succeeds and declares
/// that file too (G6d's "no exclusion" behavior) — verified end-to-end
/// by feeding both into `build`. Neither dependency's content is
/// embedded.
#[test]
fn resolve_export_fields_tier3_message_declares_its_file_as_a_dependency_no_exclusion() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();

    let other_file_field = field_by_number(&fields, 2);
    assert_eq!(other_file_field.r#type, Type::Message);
    assert_eq!(other_file_field.type_name.as_deref(), Some(".other.Msg"));
    assert!(other_file_field.referenced_file.is_some());

    let own_file_field = field_by_number(&fields, 3);
    assert_eq!(own_file_field.r#type, Type::Message);
    assert_eq!(own_file_field.type_name.as_deref(), Some(".test.OwnType"));
    assert!(own_file_field.referenced_file.is_some());

    let fds = export_descriptor::build("Msg", fields);
    assert_eq!(fds.file.len(), 1, "no dependency's content is embedded");
    let dependency = &fds.file[0].dependency;
    assert!(dependency.contains(&"other.proto".to_string()));
    assert!(
        dependency.contains(&"outer.proto".to_string()),
        "got: {dependency:?}"
    );
}

/// G6c tier 1: an active `PathField` override at the cursor's own path
/// retypes the matching field.
#[test]
fn resolve_export_fields_tier1_path_field_override_retypes_the_field() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: app.positional_path(app.cursor),
            field: 4,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let retyped = field_by_number(&fields, 4);
    assert_eq!(retyped.r#type, Type::Message);
    assert_eq!(retyped.type_name.as_deref(), Some(".other.Msg"));
}

/// G6c tier 2: an active `FqdnField` override at the cursor's own
/// `type_fqdn` is used only when no matching `PathField` override
/// exists for that field.
#[test]
fn resolve_export_fields_tier2_fqdn_field_override_applies_without_a_path_match() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::FqdnField {
            fqdn: "test.Outer".to_string(),
            field: 6,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let retyped = field_by_number(&fields, 6);
    assert_eq!(retyped.r#type, Type::Message);
    assert_eq!(retyped.type_name.as_deref(), Some(".other.Msg"));
}

/// N4: an override entry at a path other than the cursor's has no
/// effect — field 8's tier-4 guess still applies untouched.
#[test]
fn resolve_export_fields_an_override_at_a_different_path_has_no_effect() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: "/99".to_string(),
            field: 8,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let untouched = field_by_number(&fields, 8);
    assert_eq!(untouched.r#type, Type::Int64);
    assert!(untouched.type_name.is_none());
}

/// An active override with target `None` (raw) turns that field into
/// `bytes`.
#[test]
fn resolve_export_fields_a_raw_override_turns_the_field_into_bytes() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: app.positional_path(app.cursor),
            field: 7,
        },
        None,
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let raw = field_by_number(&fields, 7);
    assert_eq!(raw.r#type, Type::Bytes);
    assert!(raw.type_name.is_none());
}

/// Two live children sharing one field_number collapse to one
/// exported field, `LABEL_REPEATED` — and, since field 8 is
/// undeclared in the schema, its type is tier 4's `WT_VARINT` ->
/// `int64` guess.
#[test]
fn resolve_export_fields_repeated_children_collapse_to_one_repeated_field() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    assert_eq!(fields.iter().filter(|f| f.number == 8).count(), 1);
    let repeated = field_by_number(&fields, 8);
    assert_eq!(repeated.label, Label::Repeated);
    assert_eq!(repeated.r#type, Type::Int64);
}

/// G6c tier 4: primitive-only children, no schema, no overrides —
/// each child's guessed keyword matches G6c's table, `dependency` is
/// empty (the `WT_VARINT` case is also covered above via field 8;
/// this exercises the same tier-4 table entry through a minimal
/// single-field cursor).
#[test]
fn resolve_export_fields_tier4_guesses_from_wire_type() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let guessed = field_by_number(&fields, 8);
    assert_eq!(guessed.r#type, Type::Int64);
    let fds = export_descriptor::build("Msg", vec![]);
    assert_eq!(fds.file[0].dependency, Vec::<String>::new());
}

/// A `WT_START_GROUP` child with no resolvable/overridden type is an
/// error.
#[test]
fn resolve_export_fields_untyped_group_child_is_an_error() {
    let app = export_fields_group_error_fixture();
    let err = app.resolve_export_fields(app.cursor).unwrap_err();
    assert!(err.contains("untyped"), "got: {err}");
}

/// Cursor node is a scalar leaf (not `is_message`): `export_descriptor_
/// bytes` returns the "not a message/group" error without calling
/// `resolve_export_fields` at all.
#[test]
fn export_descriptor_bytes_on_a_scalar_leaf_cursor_is_an_error() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(id_idx);
    assert!(!app.tree[app.cursor].span.is_message);
    let err = app.export_descriptor_bytes(false).unwrap_err();
    assert!(err.contains("not a message/group"), "got: {err}");
}

/// Fixture shared by the preview-budget tests below: a `Holder` message
/// with one `bytes blob = 1` field carrying `payload` verbatim as its
/// raw interior. Returns the ready-to-splice `App` and `blob`'s own
/// tree index, with `override_target` already set to it.
fn preview_budget_fixture_bytes(payload: &[u8]) -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN};

    use crate::decode::{decode, DescriptorContext, RootType};

    // `Empty` has no declared fields, so retyping `blob` to it makes
    // every entry of the reinterpreted payload land as an unknown
    // numeric field — which is exactly the pathological shape spec 0174
    // bounds: the byte budget applies to the raw input regardless of
    // whether anything in it resolves against the schema.
    let empty_msg = DescriptorProto {
        name: Some("Empty".to_string()),
        ..Default::default()
    };
    let holder_msg = DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("blob".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Bytes as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    // The other candidate type `blob` gets retyped to: unlike `Empty` it
    // resolves the interior into real nested *messages*, which is what
    // spec 0174 G3 is about — the surviving prefix must keep its nesting,
    // not collapse into one bytes line.
    let inner_msg = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("v".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int64 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let wrapper_msg = DescriptorProto {
        name: Some("Wrapper".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("items".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Inner".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_preview_budget.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![holder_msg, empty_msg, wrapper_msg, inner_msg],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (`static COUNTER`, matching `support.rs`'s
    // convention): the preview-budget tests all call this fixture and
    // run concurrently as separate test-binary threads, so a fixed
    // filename would race on `write`/`load`/`remove_file`.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-preview-budget-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let mut blob = Vec::new();
    write_tag(1, WT_LEN, &mut blob);
    write_varint(payload.len() as u64, &mut blob);
    blob.extend_from_slice(payload);

    let decoded = decode(&blob, &mut ctx, RootType::Named("test.Holder"), 2).unwrap();
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );

    let blob_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("tree must contain the blob field");
    app.override_target = Some(blob_idx);

    (app, blob_idx)
}

/// `preview_budget_fixture_bytes` with an interior of `field_count`
/// repetitions of a 2-byte `field 1 (varint) = 1` entry — so the
/// interior is exactly `2 * field_count` bytes, and every *even* cut
/// offset lands on a field boundary (no straddler) while every odd one
/// lands mid-field.
fn preview_budget_fixture(field_count: usize) -> (App, usize) {
    use prototext_core::helpers::{write_tag, write_varint, WT_VARINT};

    let mut payload = Vec::with_capacity(field_count * 2);
    for _ in 0..field_count {
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    preview_budget_fixture_bytes(&payload)
}

/// Number of `...` truncation markers (spec 0174 §S4).
fn ellipsis_line_count(lines: &[String]) -> usize {
    lines.iter().filter(|l| l.trim() == "...").count()
}

/// `lines` with each line's indentation and trailing `#@` annotation
/// stripped, so the assertions below read against the prototext itself.
fn bare_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.split("  #@").next().unwrap_or(l).trim().to_string())
        .collect()
}

/// Render `idx` as `target` the way a live preview actually does it —
/// through `preview_override_highlight`, which holds the result as an
/// overlay (spec 0185 S3) and never touches the document.
///
/// Spec 0210 S1: the truncation tests below used to *splice* the preview
/// in. They cannot any more, and nothing in production ever did: a
/// truncated render carries the `...` marker, which deliberately has no
/// `NodeSpan` (spec 0174 §S4), so a document holding one has a body line
/// that no node claims — and a node counting its own lines has nowhere to
/// put such a line. The byte budget applies only under `is_preview`, and
/// the one production caller of `splice_override` passes `false`, so the
/// only way to reach that state was a test.
fn preview_lines(app: &mut App, idx: usize, target: &str) -> Vec<String> {
    app.override_target = Some(idx);
    app.override_candidates = vec![(target.to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();
    match app.preview_overlay.as_ref() {
        Some(o) => o.lines.clone(),
        None => panic!("a preview of {target} must render: {}", app.message),
    }
}

/// Spec 0174 (superseding spec 0163): a candidate type structurally
/// mismatched against a large raw payload can make the recursive-descent
/// decoder mis-parse arbitrary bytes into a pathologically large
/// synthetic tree (observed on a real ~1.1MB field: over a million spans
/// from a single splice). `App::override_preview_byte_budget` bounds
/// this at the *input*: a *live preview* (`is_preview: true`) hands the
/// renderer at most that many interior bytes, so the decode, the render,
/// the span count and the line count are all bounded together, and the
/// render completes (no hang/panic) with a visible `...` marker in place
/// of the omitted remainder. A confirmed override (`is_preview: false`)
/// is intentionally exempt — see the companion test below.
#[test]
fn preview_on_a_pathological_candidate_is_bounded_by_the_byte_budget() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    // The span count is the quantity that reached a million on the real
    // blob this spec came from, so it is asserted on directly rather than
    // through the arena a splice would have grown from it.
    let (_, _, rendered) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("a pathological candidate render must still complete");

    // The budget admits at most one node per two interior bytes, i.e. a
    // quarter of `field_count` here.
    assert!(
        rendered.spans.len() < field_count / 2,
        "the render's footprint must be bounded by the byte budget, not \
         track the mis-parsed field count: spans={} field_count={field_count}",
        rendered.spans.len()
    );
    assert_eq!(
        ellipsis_line_count(&rendered.lines),
        1,
        "a truncated preview must show exactly one `...` marker"
    );
}

/// Companion to the test above (spec 0174 G5): the same pathological
/// candidate, but spliced as a *confirmed* override (`is_preview:
/// false`) rather than a live preview — must render completely, with no
/// truncation and no `...`, since this is the content that actually gets
/// shown as the real override, not a speculative guess.
#[test]
fn confirmed_override_is_not_truncated() {
    let field_count = App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT * 2;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);

    app.splice_override(blob_idx, Some("test.Empty".to_string()), false, None)
        .expect("a confirmed override splice must complete");

    assert!(
        app.tree.len() >= field_count,
        "a confirmed override must render completely, not be truncated by \
         the preview-only byte budget: tree.len()={} field_count={field_count}",
        app.tree.len()
    );
    assert_eq!(
        ellipsis_line_count(&app.lines),
        0,
        "a confirmed override must show no truncation marker"
    );
}

/// Spec 0174 §S2: `App::override_preview_byte_budget` is a plain field,
/// not a fixed constant — setting it to a custom value (as `main.rs`'s
/// `--override-preview-byte-budget` does) must actually change where a
/// live preview cuts, not just the default.
#[test]
fn preview_respects_a_custom_byte_budget() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    // 20 bytes / 2 bytes per entry = 10 entries, well under the 50 the
    // untruncated payload would have produced.
    assert_eq!(
        bare_lines(&lines).iter().filter(|l| *l == "1: 1").count(),
        10,
        "a lower custom budget must be honored, not fall back to the \
         default: lines={lines:?}"
    );
    assert_eq!(ellipsis_line_count(&lines), 1);
}

/// Spec 0174 G3: the cut is on the *input* bytes, so whatever survives
/// it is decoded and rendered exactly as it would have been in the
/// untruncated document — the entries before the cut keep their full
/// nesting and their declared types, rather than collapsing into a
/// single opaque bytes line the way naive field-level truncation would
/// leave them.
#[test]
fn preview_renders_complete_nested_fields_up_to_the_cut() {
    use prototext_core::helpers::{write_tag, write_varint, WT_LEN, WT_VARINT};

    // 20 repetitions of `items { v: 1 }` — 4 bytes each.
    let mut payload = Vec::new();
    for _ in 0..20 {
        write_tag(1, WT_LEN, &mut payload);
        write_varint(2, &mut payload);
        write_tag(1, WT_VARINT, &mut payload);
        write_varint(1, &mut payload);
    }
    let (mut app, blob_idx) = preview_budget_fixture_bytes(&payload);
    // A multiple of 4 => the cut lands exactly on an entry boundary.
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Wrapper");

    // Everything below `blob`'s own header, minus the marker.
    let interior: Vec<String> = bare_lines(&lines)
        .into_iter()
        .skip(1)
        .filter(|l| *l != "...")
        .collect();
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..5 {
        expected.extend(["items {", "v: 1", "}"].map(str::to_string));
    }
    expected.push("}".to_string()); // `blob`'s own closing brace.
    assert_eq!(
        interior, expected,
        "the surviving entries must keep their nesting and declared \
         types: lines={lines:?}"
    );
}

/// Spec 0174 G4: cutting mid-entry makes the renderer emit its own
/// malformity annotation for the straddling bytes — which is an artifact
/// of *our* cut, not of the document, so it must never reach the user.
/// §S4 replaces that line with the plain `...` marker.
#[test]
fn preview_shows_no_malformity_marker() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    // Odd budget => the cut lands mid-entry, between an entry's tag and
    // its varint payload.
    app.override_preview_byte_budget = 21;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert!(
        !lines.iter().any(|l| l.contains("TRUNCATED_BYTES")
            || l.contains("MALFORMED")
            || l.contains("UNEXPECTED_EOF")),
        "no malformity marker may leak out of a preview: lines={lines:?}"
    );
    assert_eq!(ellipsis_line_count(&lines), 1);
}

/// Spec 0174 §S4: the `...` is the *last* thing inside the truncated
/// node — just before its closing brace — so it reads as "and there is
/// more below", not as a sibling of what follows.
#[test]
fn truncated_preview_ends_with_an_ellipsis_line() {
    let field_count = 50;
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    app.override_preview_byte_budget = 20;

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    let marker = lines
        .iter()
        .position(|l| l.trim() == "...")
        .expect("a truncated preview must carry a `...` marker");
    assert_eq!(
        lines[marker + 1].trim(),
        "}",
        "the marker must sit immediately before the node's closing brace: \
         lines={lines:?}"
    );
    // S4: the marker line carries no styles and no `NodeSpan` — it is
    // not selectable, not navigable, not part of any span range. Spec
    // 0187 S2 keeps the "no styles" half true by blanking the marker in
    // `window_text`, so the highlighter never sees a row that is not
    // prototext; the row still exists, so the buckets stay one-to-one
    // with the window.
    let window: Vec<DisplayRow> = (0..lines.len()).map(DisplayRow::Overlay).collect();
    app.refresh_window_styles(&window);
    assert_eq!(app.window_styles.len(), window.len());
    assert!(app.window_styles[marker].is_empty());
    // Spec 0210 S1: asked of the render's own spans rather than of the
    // document, which never holds a marker (see `preview_lines`).
    let (_, _, rendered) = app
        .render_node_as(blob_idx, Some("test.Empty"), true)
        .expect("the same render must succeed twice");
    // An enclosing message's span legitimately spans the marker; what may
    // not exist is a node whose *own* header or footer that line is, since
    // that is what makes a line selectable.
    assert!(
        !rendered
            .spans
            .iter()
            .any(|s| s.text_range.start == marker || s.text_range.end == marker + 1),
        "no span may own the marker line: {:?}",
        rendered.spans
    );
}

/// Spec 0174 G4's converse: a preview that fits within the budget is
/// byte-for-byte the confirmed rendering — no marker, nothing to
/// mistake for missing content.
#[test]
fn untruncated_preview_has_no_ellipsis_line() {
    let (mut app, blob_idx) = preview_budget_fixture(10);

    let lines = preview_lines(&mut app, blob_idx, "test.Empty");

    assert_eq!(
        ellipsis_line_count(&lines),
        0,
        "an untruncated preview must show no marker: lines={lines:?}"
    );
}

/// Spec 0174 §S3 `TruncShape::CharBoundary`: a `string` target is cut at
/// the last UTF-8 character boundary at or before the budget, never
/// mid-character — otherwise the renderer would see (and flag) invalid
/// UTF-8 that the document does not actually contain.
#[test]
fn preview_of_a_long_string_stays_valid_utf8() {
    // 50 two-byte characters; an odd budget guarantees the naive cut
    // would land mid-character.
    let payload = "é".repeat(50);
    let (mut app, blob_idx) = preview_budget_fixture_bytes(payload.as_bytes());
    app.override_preview_byte_budget = 21;

    let lines = preview_lines(&mut app, blob_idx, "string");

    let value = lines
        .iter()
        .find(|l| l.contains('"'))
        .expect("the string value must be rendered");
    assert!(
        value.contains(&"é".repeat(10)) && !value.contains(&"é".repeat(11)),
        "the cut must fall back to the last character boundary at or \
         before the budget: {value}"
    );
    assert!(
        !value.contains("INVALID_STRING"),
        "the cut must never leave a partial character behind: {value}"
    );
    assert_eq!(ellipsis_line_count(&lines), 1);
}

/// Spec 0174 §S3 `TruncShape::Never`: a singular numeric value is
/// bounded by construction (10 bytes at most), so it is never cut — a
/// budget lower than its own width must not corrupt it.
#[test]
fn preview_of_a_singular_varint_is_never_truncated() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.override_target = Some(id_idx);
    app.override_preview_byte_budget = 1;

    let lines = preview_lines(&mut app, id_idx, "int64");

    assert_eq!(
        ellipsis_line_count(&lines),
        0,
        "a singular varint must never be truncated: lines={lines:?}"
    );
    assert!(
        bare_lines(&lines).iter().any(|l| l == "id: 5"),
        "the value must survive intact: lines={lines:?}"
    );
}

/// Spec 0174 §S3 (Offsets): rewriting the LEN framing can shrink the
/// length varint — here a 3-byte original (16 400 bytes) becomes a
/// 2-byte one (4096) — so every span the renderer reports sits one byte
/// earlier than in `self.blob`. `splice_override` folds that shift into
/// `byte_offset`; if it did not, every child's `raw_range` would be off
/// by one and point at garbage.
#[test]
fn preview_child_spans_survive_the_length_prefix_shift() {
    let field_count = 8_200; // 16 400 interior bytes => 3-byte length varint.
    let (mut app, blob_idx) = preview_budget_fixture(field_count);
    // Spec 0210 S1: the one truncation test that still has to splice,
    // because `span_shift` is folded in by the splice and nowhere else.
    // The resulting document holds the `...` marker, a line no node claims
    // (spec 0174 §S4), which is precisely what the line counters cannot
    // represent — so the counter check is turned off for it. Nothing in
    // production reaches this state: the budget applies only under
    // `is_preview`, and `render_overrides` splices with `false`.
    app.verify_repair = false;

    app.splice_override(blob_idx, Some("test.Empty".to_string()), true, None)
        .expect("a preview splice must complete");

    // Walked through the sibling chain, not by scanning for `parent ==
    // blob_idx`: the pushed copy of the local root is deliberately left
    // orphaned (it carries `blob_idx`'s own parent link) and is never
    // part of the live tree.
    let mut child = app.tree[blob_idx].first_child;
    let mut count = 0usize;
    while let Some(c) = child {
        let r = app.tree[c].span.raw_range.clone();
        assert_eq!(
            &app.blob[r.clone()],
            &[0x08u8, 0x01],
            "child {c}'s raw_range {r:?} must still point at its own \
             on-the-wire bytes"
        );
        count += 1;
        child = app.tree[c].next_sibling;
    }
    assert_eq!(
        count,
        App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT / 2,
        "the whole budget's worth of entries must be rendered"
    );
}

/// Seed `app` with `n` synthetic committed lines, so that the patch
/// tests below have a document to merge against without depending on
/// whatever the fixture happened to render.
fn seed_committed_lines(app: &mut App, n: usize) {
    app.lines = (0..n).map(|i| format!("old{i}")).collect();
}

fn original_patch(range: Range<usize>, lines: &[&str]) -> LinePatch {
    LinePatch {
        target: LinePatchTarget::Original(range.clone()),
        global_start: range.start,
        children_base_shift: 0,
        lines: lines.iter().map(|l| l.to_string()).collect(),
    }
}

/// Flaw C2 (worklist W2): `materialize_line_patches`'s forward merge is
/// only correct for ascending, non-overlapping ranges. That used to rest
/// entirely on *callers* queueing in order, guarded by a
/// `debug_assert!` — which a project built exclusively with `--release`
/// never compiles, so the guard was absent from every binary anyone ran.
/// The requirement was violable at a distance, and was in fact violated
/// (the 2026-07-25 `doc_next` cycle bug presented as a bare slice panic
/// in this loop rather than naming the splice that queued the bad
/// patch). Ordering is now the merge's own business: it sorts. This test
/// queues back-to-front and demands the correct merge anyway, and it
/// runs under `--release` like every other test here, which is the whole
/// point.
#[test]
fn top_level_line_patches_merge_correctly_when_queued_out_of_order() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 6);

    app.pending_line_patches = vec![
        original_patch(4..6, &["B"]),
        original_patch(1..2, &["A0", "A1"]),
    ];
    app.materialize_line_patches();

    assert_eq!(app.lines, ["old0", "A0", "A1", "old2", "old3", "B"]);
    assert!(app.pending_line_patches.is_empty());
}

/// Flaw C2, the nested half: a patch's `Nested` children are resolved by
/// the same forward merge, against the parent patch's own lines, and so
/// need the same ordering guarantee.
#[test]
fn nested_line_patches_merge_correctly_when_queued_out_of_order() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 4);

    let nested = |parent: usize, range: Range<usize>, line: &str| LinePatch {
        target: LinePatchTarget::Nested(parent, range),
        global_start: 0,
        children_base_shift: 0,
        lines: vec![line.to_string()],
    };
    app.pending_line_patches = vec![
        original_patch(1..3, &["p0", "p1", "p2", "p3"]),
        nested(0, 2..3, "Y"),
        nested(0, 0..1, "X"),
    ];
    app.materialize_line_patches();

    assert_eq!(app.lines, ["old0", "X", "p1", "Y", "p3", "old3"]);
}

/// Flaw C2: overlap is a genuine contradiction rather than a permutation
/// problem, so it stays an assertion — but a real one, naming the
/// offending pair instead of surfacing as `slice index starts at 3 but
/// ends at 2` inside the merge loop.
#[test]
#[should_panic(expected = "overlapping top-level line patches")]
fn overlapping_line_patches_panic_with_a_directed_message() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 6);

    app.pending_line_patches = vec![original_patch(0..3, &["A"]), original_patch(2..4, &["B"])];
    app.materialize_line_patches();
}

/// Spec 0186 S3, the crux. A splice grows every ancestor's
/// `lines_total`, so an ancestor's *footer* line moves while its
/// *header* line does not — the asymmetry the old line maps had to be
/// repaired by line index rather than by subtree, and the one the
/// counters now have to reproduce for free.
///
/// Note what this asserts beyond "the new footer resolves": that
/// *nothing* still claims the old one. A stale duplicate passes the
/// weaker check and then navigates the user to the wrong node.
#[test]
fn a_deeply_nested_splice_moves_its_ancestors_footers_without_leaving_stale_entries() {
    let mut app = nested_any_fixture();
    // The auto-expanded `Any` itself, three message levels down —
    // deliberately not its `acme.Payload` value, every alternative
    // rendering of which happens to occupy the same three lines in this
    // fixture, so no ancestor footer would move at all.
    let payload = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload"))
        .expect("the Any's value must resolve to acme.Payload");
    let target = app.tree[payload]
        .parent
        .expect("the payload must sit inside the Any");

    // Only ancestors that actually have a footer line of their own — a
    // single-line node's `end - 1` is its header, and the full rebuild
    // records no footer entry for it either.
    let mut ancestors: Vec<(usize, usize)> = Vec::new();
    let mut p = app.tree[target].parent;
    while let Some(pi) = p {
        let r = &app.node_lines(pi);
        if r.end - 1 > r.start {
            ancestors.push((pi, r.end - 1));
        }
        p = app.tree[pi].parent;
    }
    assert!(
        ancestors.len() >= 3,
        "the fixture must be deep enough to exercise the ancestor walk, \
         got {} ancestors with footers",
        ancestors.len()
    );

    // Collapse the whole `Any` subtree into a single mismatched scalar
    // line, so every one of those ancestor footers moves up.
    let lines_before = app.lines.len();
    app.splice_override(target, Some("int32".to_string()), false, None)
        .expect("re-typing the Any as a scalar must succeed (as a mismatch)");
    assert!(
        app.lines.len() < lines_before,
        "the fixture must actually shrink the document: {:#?}",
        app.lines
    );

    for &(ancestor, old_footer) in &ancestors {
        let new_footer = app.node_lines(ancestor).end - 1;
        assert_ne!(
            new_footer, old_footer,
            "the fixture is not proving anything unless ancestor \
             {ancestor}'s footer actually moved"
        );
        assert_eq!(
            app.node_at_footer_line(new_footer),
            Some(ancestor),
            "ancestor {ancestor}'s footer must be mapped at its new line \
             {new_footer}"
        );
        assert_ne!(
            app.node_at_footer_line(old_footer),
            Some(ancestor),
            "a stale entry survived at ancestor {ancestor}'s *old* footer \
             line {old_footer}"
        );
    }
}

/// Spec 0186 S3's asymmetry: the shift walk is guarded by `delta != 0`,
/// the map repair must not be. Re-splicing a node as the type it already
/// has shifts nothing at all, but the repair still dropped every entry
/// at or after the patch and owes them back.
#[test]
fn a_zero_delta_splice_still_repairs_the_line_maps() {
    let mut app = nested_any_fixture();
    let target = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload"))
        .expect("the Any's value must resolve to acme.Payload");
    let lines_before = app.lines.clone();

    app.splice_override(target, Some("acme.Payload".to_string()), false, None)
        .expect("re-splicing a node as its current type must succeed");

    assert_eq!(
        app.lines, lines_before,
        "this is only the zero-delta case if the content is unchanged"
    );
    let mut cur = Some(app.first_node);
    while let Some(c) = cur {
        let r = app.node_lines(c);
        assert_eq!(
            app.node_at_header_line(r.start),
            Some(c),
            "node {c}'s header line {} lost its map entry to a repair \
             that ran only because lines moved",
            r.start
        );
        if r.end - 1 > r.start {
            assert_eq!(
                app.node_at_footer_line(r.end - 1),
                Some(c),
                "node {c}'s footer line {} lost its map entry",
                r.end - 1
            );
        }
        cur = app.tree[c].doc_next;
    }
}

/// Spec 0186 S2 / spec 0188 G1: a batch that queues no patches leaves
/// `pending_patch_min_line` at `None`, and the finalizer then returns
/// without touching anything.
///
/// Spec 0186 read that `None` as "no safe lower bound" and rebuilt the
/// whole document from it. It is the opposite: no patch means no text
/// was replaced and no span was shifted, so there is nothing to
/// rebuild. What this test pins is that the skip really is a no-op —
/// the lines, the visible rows and every line's owner must come out
/// identical.
#[test]
fn a_batch_that_patches_nothing_leaves_the_document_intact() {
    let mut app = nested_any_fixture();
    let lines_before = app.lines.clone();
    let rows_before = app.visible_row_count();
    let owners_before = line_owners(&app);

    // The fixture's own construction already ran this pass, so a second
    // one has nothing left to resettle.
    let cursor = app.cursor;
    app.render_overrides(cursor);

    assert!(
        app.pending_patch_min_line.is_none(),
        "a second identical pass must queue no patches"
    );
    assert_eq!(app.lines, lines_before);
    assert_eq!(app.visible_row_count(), rows_before);
    assert_eq!(line_owners(&app), owners_before);
}

/// Spec 0186 S4 retained the prefix of `visible_rows` with a
/// `partition_point`, which was only sound while the vector was sorted
/// ascending. Spec 0210 S2 deletes the vector: the visible rows are
/// walked from the counters, so ascending order is a property of the
/// walk rather than of a stored array. What is left worth pinning is
/// that the walk stays within the spliced document and enumerates every
/// row exactly once, in order.
#[test]
fn the_visible_row_walk_stays_ordered_across_a_splice() {
    fn walked(app: &App) -> Vec<usize> {
        app.visible_window(0, app.visible_row_count())
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    let mut app = nested_any_fixture();
    let before = walked(&app);
    assert!(
        before.windows(2).all(|w| w[0] < w[1]),
        "the visible rows must come out ascending before the splice"
    );

    let target = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload"))
        .expect("the Any's value must resolve to acme.Payload");
    app.splice_override(target, None, false, None)
        .expect("collapsing the payload to raw bytes must succeed");

    let after = walked(&app);
    assert!(
        after.windows(2).all(|w| w[0] < w[1]),
        "...and after it: {after:?}"
    );
    assert!(
        after.iter().all(|&l| l < app.lines.len()),
        "no row may point past the spliced document"
    );
    assert_eq!(
        after.len(),
        app.visible_row_count(),
        "the walk must yield exactly as many rows as the counters claim"
    );
}

/// Spec 0210 test-plan items 12 and 14: a subtree that follows a splice
/// and that the override walk *prunes* must still report the right lines.
///
/// This is the shape S11's deleted code existed for. The walk used to
/// carry the splice's line-count delta down the tree and, on reaching a
/// pruned child, walk that child's whole subtree by `doc_next` to shift
/// every stored range inside it — which on a real document meant
/// shifting most of the arena for an override near the top. Now nothing
/// is shifted, because nothing is stored: `node_lines` sums the
/// counters, and a count does not care what happened above it.
///
/// So the assertions below are unchanged from when they caught the
/// original bug (fold handles and heat cues landing on the closing brace
/// instead of the opening one, everywhere below an override), and that
/// is the point — S11's claim is that it removes work with no observable
/// effect. Running under `verify_repair` also puts item 1's recount
/// oracle on the exact path the deleted arm used to take.
///
/// Retyping `head` from `Wrap` to `Blob` reads the same four bytes as
/// one string line instead of a nested `leaf { v: 5 }`, shortening the
/// document by two lines. `tail` renders identically either way, so the
/// walk prunes it — and its `leaf`/`v` interior is what used to be left
/// two lines behind.
#[test]
fn a_pruned_subtree_after_a_splice_still_reports_its_own_lines() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, _head, tail, tail_leaf, tail_v) = pruned_tail_fixture();
    let root = app.first_node;

    assert_eq!(
        app.lines,
        vec![
            "1 {  #@ Outer = 1",
            "  head {  #@ Wrap = 1",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "  tail {  #@ Wrap = 2",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "}",
        ],
        "fixture must start out as two identically shaped Wraps"
    );

    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/1".to_string(),
        },
        Some("test.Blob".to_string()),
    );
    app.render_overrides(root);

    assert_eq!(
        app.lines,
        vec![
            "1 {  #@ Outer = 1",
            "  head {  #@ Blob = 1",
            "    leaf: \"\\010\\005\"  #@ string = 1",
            "  }",
            "  tail {  #@ Wrap = 2",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "}",
        ],
        "retyping head as Blob must shorten the document by two lines"
    );

    // The premise: had the walk descended into `tail`, the recursion
    // would have shifted its interior and this test would prove nothing.
    // Marks persist across batches (spec 0188 S4), so this reads the
    // decision the batch actually made.
    assert!(
        !app.descend[tail],
        "the walk must have pruned `tail` for this test to mean anything"
    );

    for (node, name, want) in [
        (tail, "tail", 4..9),
        (tail_leaf, "tail.leaf", 5..8),
        (tail_v, "tail.leaf.v", 6..7),
    ] {
        assert_eq!(
            app.node_lines(node),
            want,
            "{name} reports pre-splice line numbers, so the pruned \
             subtree's interior did not follow the splice"
        );
    }

    // And the symptom itself: the fold handle and the heat cue come from
    // the header/footer resolution alone, so this is the assertion a
    // user would recognize.
    for (line, node, name) in [
        (4, tail, "tail's `{` line"),
        (5, tail_leaf, "tail.leaf's `{` line"),
        (6, tail_v, "tail.leaf.v's own scalar line"),
    ] {
        assert_eq!(app.node_at_header_line(line), Some(node), "{name}");
    }
    for (line, node, name) in [
        (8, tail, "tail's `}` line"),
        (7, tail_leaf, "tail.leaf's `}` line"),
    ] {
        assert_eq!(app.node_at_footer_line(line), Some(node), "{name}");
    }
    assert_eq!(
        app.node_at_footer_line(6),
        None,
        "a scalar line must never carry a fold handle"
    );
}

/// A seeded random walk of override activations and toggles over every
/// document shape the suite has a fixture for.
///
/// The hand-written tests around it each pin one situation someone
/// already thought of. This one exists for the situations nobody did —
/// and the bug it was written after is exactly that kind: it needed a
/// splice *followed by* a subtree the walk prunes, a combination no
/// fixture had, because a fixture is built to demonstrate one mechanism
/// and this bug lives in the interaction of two.
///
/// It asserts nothing itself. Everything it can catch is asserted by
/// `finalize_override_batch` under `verify_repair` — the span/text
/// consistency oracle and the spec 0186 G3 map equivalence — so this is
/// purely an input generator, and any new invariant hung off that
/// finalizer is exercised by these sequences for free.
///
/// Deliberately seeded rather than randomly seeded: a failure has to
/// replay exactly, and a test that fails one run in twenty is a test
/// people learn to re-run.
#[test]
fn randomized_override_sequences_keep_every_span_consistent() {
    use crate::override_pane::OverrideOrigin;

    for seed in 1u64..=6 {
        for (shape, mut app) in [
            ("pruned_tail", pruned_tail_fixture().0),
            ("repeated_message", repeated_message_fixture().0),
            ("packed_run_with_tail", packed_run_with_tail_fixture().0),
            ("nested_packed_run", nested_packed_run_fixture()),
            ("nested_any", nested_any_fixture()),
            ("message_set", message_set_fixture()),
            ("nested_message_set", nested_message_set_fixture()),
        ] {
            // The candidate types are the ones this document actually
            // mentions, plus raw — so each shape is retyped into things
            // that are plausible for it rather than into a fixed list
            // that most fixtures would reject outright.
            let mut types: Vec<Option<String>> = vec![None];
            let mut cur = Some(app.first_node);
            while let Some(c) = cur {
                if let Some(t) = app.tree[c].span.type_fqdn.clone() {
                    if !types.iter().any(|k| k.as_deref() == Some(t.as_str())) {
                        types.push(Some(t));
                    }
                }
                cur = app.tree[c].doc_next;
            }

            let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut next = move || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) as usize
            };

            for step in 0..40 {
                if next() % 10 < 7 {
                    let mut pick = None;
                    for _ in 0..50 {
                        let l = next() % app.lines.len();
                        if let Some(i) = app.node_at_header_line(l) {
                            if app.can_override(i) {
                                pick = Some(i);
                                break;
                            }
                        }
                    }
                    let Some(idx) = pick else { continue };
                    let ty = types[next() % types.len()].clone();
                    let path = app.positional_path(idx);
                    // Printed, not asserted: cargo shows a failing
                    // test's stdout, so the sequence that reached the
                    // failure is in the report without costing anything
                    // on the passing runs.
                    println!("{shape} seed {seed} step {step}: {path} as {ty:?}");
                    app.overrides
                        .activate(OverrideOrigin::Path { path }, ty.clone());
                    app.render_overrides(app.first_node);
                } else if !app.overrides.entries().is_empty() {
                    let e = next() % app.overrides.entries().len();
                    println!("{shape} seed {seed} step {step}: toggle entry {e}");
                    app.overrides.toggle_active(e);
                    app.render_overrides(app.first_node);
                }
            }
        }
    }
}

/// A packed element's `packed_record_start` is a byte offset into
/// `self.blob`, read back with `parse_wiretag`/`parse_varint` by
/// `packed_record_extent`, `extract::message_payload_range` and the heat
/// cue. When a *splice* creates such an element, `prototext-core` hands
/// it back in the retyped node's own byte frame, and `splice_override`
/// has to translate it into the document's — exactly as it already does
/// for `raw_range`.
///
/// It did not, and the consequence was not a cosmetic one. On
/// `/tmp/pdb.desc`, retyping a `SourceCodeInfo.Location` and then one of
/// the `span:` elements the retype produced made `packed_record_extent`
/// parse a tag and a length out of unrelated bytes near the start of the
/// file: a 2-byte varint was replaced by a re-render of the whole 1 MB
/// blob, the tree came back with a node as its own descendant, and
/// `collect_descendants` recursed until the stack ran out. A 256 MB
/// stack did not help, which is what identified it as unbounded rather
/// than deep.
#[test]
fn a_splice_translates_packed_record_starts_into_the_documents_byte_frame() {
    use crate::override_pane::OverrideOrigin;

    let mut app = nested_packed_run_fixture();
    // The two frames must genuinely differ, or this test proves nothing:
    // `blob`'s own tag is at byte 2, so everything the retype decodes is
    // reported 2 bytes lower than it really sits.
    assert_eq!(
        &app.blob[..],
        &[0x0A, 0x09, 0x08, 0x01, 0x12, 0x05, 0x0A, 0x03, 0x05, 0x06, 0x07]
    );
    let blob_node = app.resolve_path("/2").expect("blob is /2");
    assert_eq!(app.tree[blob_node].span.raw_range, 4..11);

    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/2".to_string(),
        },
        Some("test.Payload".to_string()),
    );
    app.render_overrides(app.first_node);

    let mut elems = Vec::new();
    let mut c = app.tree[blob_node].first_child;
    while let Some(i) = c {
        elems.push(i);
        c = app.tree[i].next_sibling;
    }
    assert_eq!(elems.len(), 3, "Payload.vals decodes as three packed ints");

    for &e in &elems {
        assert_eq!(
            app.tree[e].span.packed_record_start,
            Some(6),
            "the packed record's tag is at byte 6 of the document; a \
             value of 2 is the retyped node's own frame leaking out"
        );
    }
    // The offset is only ever used by way of a re-parse, so assert the
    // thing that re-parse produces: tag 0x0A at 6, length 3 at 7,
    // payload 8..11.
    let (raw, _text) = app.packed_record_extent(&elems);
    assert_eq!(raw, 6..11);
}

/// The override arena is append-only: every splice appends a fresh copy
/// of its target's subtree and never frees the superseded one, so a
/// document-wide override costs a whole second copy of the document
/// each time it is applied *or removed*. On googleapis that is +1.4 GiB
/// per batch with the live set flat, and the third batch is
/// reproducibly killed by the OOM killer.
///
/// The guard trades that kill for a refusal. It checks headroom for one
/// more batch — the worst case appends about as many nodes as the arena
/// already holds — against half of what the machine has free.
#[test]
fn the_memory_guard_refuses_a_batch_with_no_headroom_left() {
    let (mut app, _head, _tail, _tail_leaf, _tail_v) = pruned_tail_fixture();

    let per_node = (std::mem::size_of::<TreeNode>() + 64) as u64;
    let arena = app.tree.len() as u64 * per_node;
    assert!(arena > 0, "the fixture must have decoded some nodes");

    assert_eq!(
        app.override_batch_refusal(arena * 2),
        None,
        "an arena occupying exactly half of what is free still has room \
         for one more batch"
    );

    let refusal = app
        .override_batch_refusal(arena * 2 - 2)
        .expect("one byte past half of what is free must be refused");
    assert!(
        refusal.starts_with("override skipped:"),
        "the refusal must say so in the status line rather than fail \
         silently: {refusal}"
    );
    assert!(
        refusal.contains("restart protolens"),
        "and must say what to do about it: {refusal}"
    );

    // The guard reads only quantities that are already exact, so it
    // cannot be fooled by a batch that turns out to splice nothing —
    // the case that sank an earlier estimate built from the descent
    // marks (at startup those mark the root alone, charging the batch
    // the whole document for a pass that splices nothing).
    app.memory_available = Some(u64::MAX);
    assert_eq!(app.override_batch_refusal(u64::MAX), None);
}

/// A refused batch must leave the document exactly as it was. The
/// refusal happens before `render_overrides` touches anything, so the
/// text keeps its previous rendering — recoverable, unlike being
/// killed.
#[test]
fn a_refused_batch_leaves_the_document_untouched() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, _head, _tail, _tail_leaf, _tail_v) = pruned_tail_fixture();
    let root = app.first_node;
    let before = app.lines.clone();
    let tree_before = app.tree.len();

    // Two bytes short of the arena's own size doubled: no headroom.
    let per_node = (std::mem::size_of::<TreeNode>() + 64) as u64;
    app.memory_available = Some(app.tree.len() as u64 * per_node * 2 - 2);

    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/1".to_string(),
        },
        Some("test.Blob".to_string()),
    );
    app.render_overrides(root);

    assert_eq!(app.lines, before, "a refused batch must not re-render");
    assert_eq!(
        app.tree.len(),
        tree_before,
        "and must not append to the arena either"
    );
    assert!(
        app.message.starts_with("override skipped:"),
        "the user must be told why nothing happened: {}",
        app.message
    );

    // With headroom restored the very same batch goes through, so the
    // guard refuses rather than corrupts. The entry has to be activated
    // again first: the refusal deactivated it (see
    // `a_refused_override_is_left_deactivated`), which is the whole
    // point — nothing stays marked as applied that was not applied.
    app.memory_available = Some(u64::MAX);
    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/1".to_string(),
        },
        Some("test.Blob".to_string()),
    );
    app.render_overrides(root);
    assert_ne!(app.lines, before, "the batch runs once there is room");
}

/// Spec 0202 S4: the entry the user just activated does not stay marked
/// active through a refusal.
///
/// The guard keeps refusing for the rest of the session (S2's
/// bluntness), so an entry left active would not be a momentary
/// inconsistency — it would claim, permanently and in the one place the
/// user goes to check, a rendering the document is never going to get.
/// The entry itself is kept: it is still listed, editable and savable,
/// it just no longer says it is in effect.
///
/// What is restored is the state the document actually shows, not blanket
/// deactivation: the root entry that *was* applied stays active, and a
/// previously-active entry for the same origin is put back rather than
/// left off by the activation that replaced it.
#[test]
fn a_refused_override_is_left_deactivated() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, _head, _tail, _tail_leaf, _tail_v) = pruned_tail_fixture();
    let root = app.first_node;
    let origin = OverrideOrigin::Path {
        path: "/1".to_string(),
    };

    // A first, applied override for that origin — the state the
    // document is left showing.
    app.overrides.activate(origin.clone(), None);
    app.render_overrides(root);
    let applied = app.lines.clone();

    let per_node = (std::mem::size_of::<TreeNode>() + 64) as u64;
    app.memory_available = Some(app.tree.len() as u64 * per_node * 2 - 2);

    // Now retype it, which `activate` does by deactivating the raw entry
    // and activating a typed one beside it.
    app.overrides
        .activate(origin.clone(), Some("test.Blob".to_string()));
    app.render_overrides(root);

    assert_eq!(app.lines, applied, "the refused batch rendered nothing");
    assert!(
        app.message.contains("left deactivated"),
        "the refusal must say the override was not left claiming to \
         apply: {}",
        app.message
    );

    let active: Vec<(String, Option<String>)> = app
        .overrides
        .entries()
        .iter()
        .filter(|e| e.active)
        .map(|e| (e.origin.label(), e.r#type.clone()))
        .collect();
    assert!(
        !active.contains(&("/1".to_string(), Some("test.Blob".to_string()))),
        "the refused override must not stay active: {active:?}"
    );
    assert!(
        active.contains(&("/1".to_string(), None)),
        "and the override the document does show must be active again: \
         {active:?}"
    );
    assert!(
        app.overrides
            .entries()
            .iter()
            .any(|e| e.origin == origin && e.r#type.as_deref() == Some("test.Blob")),
        "the entry itself is kept, just inactive"
    );

    // A refusal must not reach back past a batch that did render: the
    // applied entry stays applied however many overrides are refused
    // afterwards.
    app.overrides
        .activate(origin.clone(), Some("test.Blob".to_string()));
    app.render_overrides(root);
    assert!(
        app.overrides
            .entries()
            .iter()
            .any(|e| e.origin == origin && e.r#type.is_none() && e.active),
        "the second refusal must restore the same state as the first"
    );
}
