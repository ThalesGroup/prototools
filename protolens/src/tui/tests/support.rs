// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_cue::HEAT_CUE_PREVIEW;
use super::super::*;
pub(super) use prototext_core::helpers::{WT_LEN, WT_VARINT};
pub(super) use prototext_core::serialize::render_text::NodeSpan;
use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::score::load::LoadedGraph;
pub(super) use ratatui::backend::TestBackend;

/// A node's identity by *content*, not by arena index.
///
/// The arena renumbers nodes for two independent reasons — a re-splice
/// pushes a fresh copy of a subtree and abandons the old one (spec
/// 0118), and compaction relocates survivors into the holes that leaves
/// (spec 0203) — so comparing raw `app.tree` indices across either
/// operation reports differences that say nothing about what the user
/// sees. Projecting through this compares what actually
/// has to be preserved.
pub(super) type Shape = (usize, u64, Option<String>, std::ops::Range<usize>);

/// A rendered line's owner: `(line, the owning node's shape, is_footer)`.
pub(super) type LineOwner = (usize, Shape, bool);

pub(super) fn shape_of(app: &App, idx: usize) -> Shape {
    let s = &app.tree[idx].span;
    // Spec 0210 S1: the line range is derived from the counters, not
    // read off `span.text_range` — which is the range the node had when
    // the tree was built and is not repaired by a splice.
    // Spec 0212: widened back out here on purpose. `Shape` exists to be
    // printed in an assertion failure, and an `FqdnId(37)` says nothing.
    (
        s.level as usize,
        u64::from(s.field_number),
        app.fqdns.get(s.type_fqdn).map(str::to_owned),
        app.node_lines(idx),
    )
}

/// The first node in the arena whose resolved type is `fqdn`.
///
/// Spec 0212 S6: the name is interned once, here, and the ids compared.
/// Resolving each node's id back to a string inside the closure would need
/// `app.fqdns` borrowed alongside `app.tree` and would turn every one of
/// these iterator chains into an index loop. `id_of`'s miss is
/// `UNINTERNED`, which no span holds, so a name the document never
/// produced finds nothing rather than matching every typeless node.
pub(super) fn node_with_type(app: &App, fqdn: &str) -> Option<usize> {
    let want = app.fqdns.id_of(fqdn);
    app.tree.iter().position(|n| n.span.type_fqdn == want)
}

/// Whether any node in the arena has resolved type `fqdn` — the `any`
/// counterpart of `node_with_type`.
pub(super) fn has_node_with_type(app: &App, fqdn: &str) -> bool {
    node_with_type(app, fqdn).is_some()
}

/// `idx`'s resolved type name, for the assertions that want to *print* it
/// rather than match it.
pub(super) fn type_name_of(app: &App, idx: usize) -> Option<&str> {
    app.fqdns.get(app.tree[idx].span.type_fqdn)
}

/// Every node still reachable from the root, in document order.
pub(super) fn live_shapes(app: &App) -> Vec<Shape> {
    let mut out = Vec::new();
    let mut stack = vec![app.first_node];
    while let Some(i) = stack.pop() {
        out.push(shape_of(app, i));
        let mut kids = Vec::new();
        let mut c = app.tree[i].first_child();
        while let Some(ci) = c {
            kids.push(ci);
            c = app.tree[ci].next_sibling();
        }
        stack.extend(kids.into_iter().rev());
    }
    out
}

/// Every line's owner, projected through `Shape`, and whether the line
/// is that owner's footer.
///
/// Spec 0210 S2: the replacement for the `shaped_map(app,
/// &app.line_to_node)` / `shaped_map(app, &app.footer_line_to_node)`
/// pair. Both maps are gone, so the question "does every line still
/// resolve to the same node it did" is asked of `line_pos` — which is
/// the thing every reader now goes through anyway.
pub(super) fn line_owners(app: &App) -> Vec<LineOwner> {
    (0..app.lines.len())
        .filter_map(|l| {
            app.line_pos(l)
                .map(|pos| (l, shape_of(app, pos.node), pos.footer))
        })
        .collect()
}

pub(super) fn empty_app() -> App {
    let decoded = Decoded {
        lines: Vec::new(),
        tree: Vec::new(),
        root_type: "google.protobuf.Empty".to_string(),
        blob: Vec::new(),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    App::new(
        decoded,
        "empty.pb",
        PathBuf::from("empty.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    )
}

/// A single-node tree whose root is a message/group node — the
/// minimal fixture needed to exercise `t`'s override-target
/// validation (spec 0114 §1).
pub(super) fn message_node_app() -> App {
    message_node_app_with_root_candidates(Vec::new())
}

/// `message_node_app` carrying a root-type inference result on its
/// `Decoded` (spec 0168 G3) — the shape startup hands `App::new` when
/// the sweep ran, so `seed_root_heat` has something to seed from.
pub(super) fn message_node_app_with_root_candidates(
    root_candidates: crate::decode::RankedCandidates,
) -> App {
    let lines: Vec<String> = vec!["message_type {".to_string(), "}".to_string()];
    let mut fqdns = FqdnTable::new();
    let node = TreeNode {
        span: NodeSpan {
            field_number: 4,
            raw_range: 0..10,
            text_range: 0..2,
            level: 0,
            type_fqdn: fqdns.intern("google.protobuf.DescriptorProto"),
            is_message: true,
            packed_record_start: NO_PACKED_RECORD,
            wire_type: WT_LEN as u8,
        },
        parent: NO_NODE,
        first_child: NO_NODE,
        last_child: NO_NODE,
        next_sibling: NO_NODE,
        prev_sibling: NO_NODE,
        doc_next: NO_NODE,
        doc_prev: NO_NODE,
        sibling_ordinal: 1,
        lines_total: 2,
        lines_visible: 2,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines,
        tree: vec![node],
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        // Tag `0x22` = field 4 << 3 | WT_LEN(2), length varint `0x08`
        // = 8, then 8 zero payload bytes — a real, `raw_range`-
        // consistent blob, needed since spec 0132's live preview now
        // splices this node's contents at pane-open time.
        blob: vec![0x22, 0x08, 0, 0, 0, 0, 0, 0, 0, 0],
        wrapper_offset: 0,
        root_candidates,
        fqdns,
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

/// A minimal, real, in-memory scoring graph (spec 0152 test plan) —
/// `HEAT_CUE_PREVIEW` messages, each with a single `uint64` field 1 —
/// built with zero file I/O via `build_from_strings` + `Box::leak` +
/// `LoadedGraph::from_static_bytes` (as spec 0151's own notes
/// anticipated). At least `HEAT_CUE_PREVIEW` non-vetoed candidates are
/// needed for `heat_cue_for`'s `[0, HEAT_CUE_PREVIEW)` window to ever
/// be satisfiable — a single-entry graph (as `heat_worker.rs`'s own,
/// lower-level round-trip test uses) is never enough here.
pub(super) fn test_scoring_graph() -> LoadedGraph {
    let mut yaml = String::from("entries:\n");
    for i in 0..HEAT_CUE_PREVIEW {
        yaml.push_str(&format!("- Msg{i}\n"));
    }
    yaml.push_str("messages:\n");
    for i in 0..HEAT_CUE_PREVIEW {
        yaml.push_str(&format!(
            "  Msg{i}:\n    fields:\n    - number: 1\n      type: uint64\n"
        ));
    }
    let (bytes, _, _) =
        build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    LoadedGraph::from_static_bytes(bytes).expect("test graph must load")
}

/// `message_node_app` with a real scoring graph attached via
/// `DescriptorContext::for_test_with_graph` (spec 0152 test plan) —
/// for tests that need `App.ctx.graph` to be genuinely `Some`, e.g. to
/// spawn a real `HeatWorkerHandle` end-to-end.
pub(super) fn message_node_app_with_graph() -> App {
    let mut app = message_node_app();
    app.ctx = DescriptorContext::for_test_with_graph(test_scoring_graph());
    app
}

/// `n` document-order-linked scalar sibling nodes at the root level
/// (spec 0113 D16: root-level nodes are sibling-linked despite having
/// no `parent`), one line of text each — the minimal fixture for
/// exercising main-pane search (spec 0114 §4, extended from the
/// override pane), which walks `doc_next`/`doc_prev`.
pub(super) fn sibling_leaves_app(texts: &[&str]) -> App {
    let lines: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    let n = lines.len();
    let tree: Vec<TreeNode> = (0..n)
        .map(|i| TreeNode {
            span: NodeSpan {
                field_number: i as u32 + 1,
                raw_range: (i * 10) as u32..(i * 10 + 5) as u32,
                text_range: i as u32..i as u32 + 1,
                level: 0,
                type_fqdn: NO_FQDN,
                is_message: false,
                packed_record_start: NO_PACKED_RECORD,
                wire_type: WT_VARINT as u8,
            },
            parent: NO_NODE,
            first_child: NO_NODE,
            last_child: NO_NODE,
            next_sibling: TreeNode::pack((i + 1 < n).then_some(i + 1)),
            prev_sibling: TreeNode::pack(i.checked_sub(1)),
            doc_next: TreeNode::pack((i + 1 < n).then_some(i + 1)),
            doc_prev: TreeNode::pack(i.checked_sub(1)),
            sibling_ordinal: i as u32 + 1,
            lines_total: 1,
            lines_visible: 1,
            rendered_as: None,
        })
        .collect();
    let decoded = Decoded {
        lines,
        tree,
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        blob: Vec::new(),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
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

/// `Outer { repeated int32 vals = 1; }`, packed, 3 elements (`5, 6,
/// 7`), document order — spec 0124's shared fixture: gives a
/// `PathField`/`FqdnField` origin (parent path `/`, field `1`) 3
/// matches. Uses a packed *scalar* repeated field (one `NodeSpan` per
/// element, spec 0115) rather than a repeated message field, to keep
/// the fixture's tree shape simple (no nested-message decode
/// involved).
///
/// Spec 0184: the 3 elements are one wire record, so they all share
/// the single positional path `/1`. A test that needs three siblings
/// with *distinct* paths wants `repeated_message_fixture` instead.
pub(super) fn repeated_scalar_fixture() -> (App, Vec<usize>) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("vals".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_repeated_scalar.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-repeated-scalar-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // vals: field 1 (tag 0x0A, LEN/packed), length 3, payload
    // [0x05, 0x06, 0x07] (three one-byte varint elements).
    let blob = [0x0Au8, 0x03, 0x05, 0x06, 0x07];
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

    let mut items: Vec<usize> = app
        .tree
        .iter()
        .enumerate()
        .filter(|(_, n)| n.span.packed_record_start != NO_PACKED_RECORD)
        .map(|(i, _)| i)
        .collect();
    items.sort_by_key(|&i| app.positional_path(i));
    assert_eq!(items.len(), 3, "fixture must contain 3 packed elements");
    (app, items)
}

/// `Outer { repeated Item items = 1; }` with 3 `Item { int32 v = 1; }`
/// submessages — three genuinely distinct sibling nodes at `/1`, `/2`,
/// `/3` that nonetheless share a parent type and field number.
///
/// This is the shape the manage pane's ambiguity machinery (spec 0134)
/// needs: an `FqdnField`/`PathField` origin matching several nodes that
/// derive *different* `Path` origins. `repeated_scalar_fixture` used to
/// serve that role, until spec 0184 made a packed run occupy a single
/// positional ordinal — an unpacked repeated *message* field is one
/// wire record per element and so keeps distinct paths by construction.
pub(super) fn repeated_message_fixture() -> (App, Vec<usize>) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let item_desc = DescriptorProto {
        name: Some("Item".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("v".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("items".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Item".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_repeated_message.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, item_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-repeated-message-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // items: field 1 (tag 0x0A, LEN), thrice, each wrapping
    // Item { v: 5 | 6 | 7 } (field 1, tag 0x08, one-byte varint).
    let blob = [
        0x0Au8, 0x02, 0x08, 0x05, //
        0x0A, 0x02, 0x08, 0x06, //
        0x0A, 0x02, 0x08, 0x07,
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

    // Walk the *live* child chain rather than scanning the arena:
    // `App::new`'s startup `render_overrides` already resettled each
    // `Item` to its natural type, and `splice_override` abandons the
    // superseded nodes in place rather than removing them, so a plain
    // scan would also pick up three orphans.
    let root = app
        .tree
        .iter()
        .position(|n| n.parent().is_none())
        .expect("tree must have a root");
    let mut items = Vec::new();
    let mut c = app.tree[root].first_child();
    while let Some(i) = c {
        items.push(i);
        c = app.tree[i].next_sibling();
    }
    assert_eq!(items.len(), 3, "fixture must contain 3 Item submessages");
    (app, items)
}

/// `n` document-order sibling scalar (`WT_VARINT`) fields, one line
/// each, each backed by a real 2-byte tag+value encoding in `blob` —
/// unlike `sibling_leaves_app`'s synthetic, unbacked ranges, this
/// fixture's `raw_range`s are real slices of a non-empty blob, needed
/// since `prefetch_step` runs every candidate through `extract::
/// message_payload_range`, which indexes into `blob` at `raw_range.
/// start` and panics on an out-of-bounds/empty one.
///
/// Field numbers cycle through 1..=15 rather than counting up, so the
/// tag always fits the single byte written here — `n` runs into the
/// thousands for the spec 0191 budget tests, and a wider tag would need
/// a real varint. Nodes stay distinct because each has its own
/// `raw_range`, which is what keys the request queue.
pub(super) fn wide_sibling_scalars_app(n: usize) -> App {
    let lines: Vec<String> = (0..n).map(|i| format!("field_{i}: 0")).collect();
    let mut blob = Vec::new();
    let tree: Vec<TreeNode> = (0..n)
        .map(|i| {
            let field_number = (i % 15) as u32 + 1;
            let start = blob.len() as u32;
            blob.push(((field_number << 3) | WT_VARINT) as u8);
            blob.push(0);
            TreeNode {
                span: NodeSpan {
                    field_number,
                    raw_range: start..start + 2,
                    text_range: i as u32..i as u32 + 1,
                    level: 0,
                    type_fqdn: NO_FQDN,
                    is_message: false,
                    packed_record_start: NO_PACKED_RECORD,
                    wire_type: WT_VARINT as u8,
                },
                parent: NO_NODE,
                first_child: NO_NODE,
                last_child: NO_NODE,
                next_sibling: TreeNode::pack((i + 1 < n).then_some(i + 1)),
                prev_sibling: TreeNode::pack(i.checked_sub(1)),
                doc_next: TreeNode::pack((i + 1 < n).then_some(i + 1)),
                doc_prev: TreeNode::pack(i.checked_sub(1)),
                sibling_ordinal: i as u32 + 1,
                lines_total: 1,
                lines_visible: 1,
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
        fqdns: FqdnTable::new(),
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

/// `Outer { repeated int32 vals = 1; Inner tail = 2; int32 a = 3;
/// int32 b = 4; }` — a packed run of 3 elements followed by three
/// ordinary siblings. Returns `(app, elems, tail, a, b)`.
///
/// The shape spec 0184 is about: the run must occupy exactly one
/// positional ordinal (`/1`) so that `tail`/`a`/`b` sit at `/2`/`/3`/
/// `/4` whatever the run's element count and whatever its override
/// state, while `a` and `b` — two consecutive *non-packed* scalars —
/// must still get distinct ordinals of their own (the S1 trap).
pub(super) fn packed_run_with_tail_fixture() -> (App, Vec<usize>, usize, usize, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let scalar = |name: &str, number: i32| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Int32 as i32),
        ..Default::default()
    };
    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![scalar("id", 3)],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("vals".to_string()),
                number: Some(1),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("tail".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Inner".to_string()),
                ..Default::default()
            },
            scalar("a", 3),
            scalar("b", 4),
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_packed_run_with_tail.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-packed-run-with-tail-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // vals: field 1 (tag 0x0A, LEN/packed), 3 one-byte elements;
    // tail: field 2 (tag 0x12, LEN) wrapping Inner { id: 5 };
    // a: field 3 (tag 0x18, varint) = 42; b: field 4 (tag 0x20) = 43.
    let blob = [
        0x0Au8, 0x03, 0x05, 0x06, 0x07, //
        0x12, 0x02, 0x18, 0x05, //
        0x18, 0x2A, //
        0x20, 0x2B,
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

    // The live children, in document order — not an arena scan: the
    // startup `render_overrides` resettles `tail` to its natural type
    // and `splice_override` abandons the superseded nodes in place.
    let root = app
        .tree
        .iter()
        .position(|node| node.parent().is_none())
        .expect("tree must have a root");
    let mut children = Vec::new();
    let mut c = app.tree[root].first_child();
    while let Some(i) = c {
        children.push(i);
        c = app.tree[i].next_sibling();
    }
    assert_eq!(
        children.len(),
        6,
        "fixture must decode 3 packed elements plus tail, a, b"
    );
    let elems = children[..3].to_vec();
    for &e in &elems {
        assert!(
            app.tree[e].span.packed_record_start != NO_PACKED_RECORD,
            "the first three children must be the packed run"
        );
    }
    (app, elems, children[3], children[4], children[5])
}

/// `Holder { int32 pad = 1; bytes blob = 2; }` whose `blob` holds an
/// encoded `Payload { repeated int32 vals = 1; }` — i.e. a packed run
/// that only exists *after* an override retypes `blob`, and that the
/// splice therefore has to translate from the retyped node's own byte
/// frame into the document's.
///
/// `pad` is there to make that translation observable: the field being
/// retyped has to start somewhere other than byte 0, or the two frames
/// coincide and any missing translation is invisible.
///
/// Byte layout, which the tests assert against directly:
///
/// ```text
///   0: 0A 09        RootType::Named's synthetic wrapper field
///   2: 08 01        pad = 1
///   4: 12 05        blob, LEN 5            <- retyped node's tag
///   6: 0A 03        Payload.vals, packed   <- the packed record's tag
///   8: 05 06 07     the three elements
/// ```
pub(super) fn nested_packed_run_fixture() -> App {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let payload_desc = DescriptorProto {
        name: Some("Payload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("vals".to_string()),
            number: Some(1),
            label: Some(Label::Repeated as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let holder_desc = DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("pad".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("blob".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Bytes as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_nested_packed_run.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![holder_desc, payload_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-nested-packed-run-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let blob = [
        0x08u8, 0x01, //
        0x12, 0x05, //
        0x0A, 0x03, 0x05, 0x06, 0x07,
    ];
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
    app.splash = false;
    app.term_width = 120;
    app
}

/// An `App` opened over a descriptor set with no `index.rkyv` sidecar,
/// so `DescriptorContext::load` took the eager path and recorded spec
/// 0197 §S3's warning.
///
/// Returned untouched — splash still up, status line still carrying the
/// warning — because those two are exactly what the §S3 tests inspect.
pub(super) fn eager_fallback_app() -> App {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let file = FileDescriptorProto {
        name: Some("test_eager_fallback.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Inner".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!("protolens-tui-eager-fallback-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let blob = [0x08u8, 0x05];
    let decoded = decode(&blob, &mut ctx, RootType::Named("test.Inner"), 2).unwrap();
    App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    )
}

/// Builds the same `Outer { inner: Inner { id: 5 } }` fixture as
/// `enter_key_applies_override_and_closes_pane`, for the `:type-as`/
/// `:type-as-raw` command tests (spec 0114 §7).
pub(super) fn type_as_fixture() -> (App, usize, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("inner".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Inner".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_type_as.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-type-as-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner { id: 5 } }.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];
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
    // The fixture writes a bare `.pb` with no `index.rkyv` beside it, so
    // `App::new` seeds the status line with spec 0197 §S3's eager-fallback
    // warning. Clear it: tests here assert on messages *they* provoke.
    app.message.clear();

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the Inner submessage");
    let id_idx = app.tree[inner_idx]
        .first_child()
        .expect("Inner has at least one child");
    (app, inner_idx, id_idx)
}

/// `Outer { inner: Inner {} }` — same schema as `type_as_fixture`, but
/// `inner`'s payload is zero-length: a genuinely empty, still-bracketed
/// submessage (rendered as `inner {` then `}` on the next line, no
/// body in between). Regression fixture for spec 0142's fix (2026-07-
/// 18 feedback): an empty message has `first_child == None` yet is
/// still a real two-line bracketed node — must be foldable and its
/// footer line must be a reachable cursor stop, same as any other
/// message.
pub(super) fn empty_message_fixture() -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let inner_desc = DescriptorProto {
        name: Some("Inner".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("inner".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Inner".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_empty_message.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-empty-message-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner {} } — field 1 (LEN), length 0, no payload.
    let blob = [0x0Au8, 0x00];
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

    let inner_idx =
        node_with_type(&app, "test.Inner").expect("tree must contain the empty Inner submessage");
    assert!(
        app.tree[inner_idx].first_child().is_none(),
        "fixture must exercise the no-children case"
    );
    (app, inner_idx)
}

/// `Outer3 { durability: Durability = 0 (EPHEMERAL) }` — a scalar
/// enum-typed field, for the enum-inclusive `natural_type` regression
/// tests (2026-07-18 feedback: `t` then `Esc` on an enum field, and
/// `t`'s initial highlight/mode on an enum field with no active
/// override).
pub(super) fn enum_field_fixture() -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
        FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let durability_enum = EnumDescriptorProto {
        name: Some("Durability".to_string()),
        value: vec![
            EnumValueDescriptorProto {
                name: Some("EPHEMERAL".to_string()),
                number: Some(0),
                ..Default::default()
            },
            EnumValueDescriptorProto {
                name: Some("PERSISTENT".to_string()),
                number: Some(1),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer3".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("durability".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Enum as i32),
            type_name: Some(".test.Durability".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_enum_field.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc],
        enum_type: vec![durability_enum],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-enum-field-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer3 { durability: EPHEMERAL (0) } — field 1 (tag 0x08),
    // varint value 0.
    let blob = [0x08u8, 0x00];
    let decoded = decode(&blob, &mut ctx, RootType::Named("test.Outer3"), 2).unwrap();
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

    let durability_idx = app
        .tree
        .iter()
        .position(|n| n.span.field_number == 1)
        .expect("tree must contain the durability field");
    (app, durability_idx)
}

/// `Outer2 { grp: MyGroup { id: 5 } }`, with `grp` declared as a
/// genuine schema wire-group field (`Type::Group`) — unlike
/// `message_set_fixture`'s auto-expanded MessageSet group items,
/// this is directly schema-resolved from the start. Also registers
/// a same-shaped sibling type `NewGroup` to override `grp` into
/// (spec 0122 Test Plan item 2).
pub(super) fn group_type_fixture() -> (App, usize) {
    // START_GROUP(5), id=5, END_GROUP(5) — minimal tag encoding.
    group_type_fixture_with_blob(&[0x2Bu8, 0x08, 0x05, 0x2Cu8])
}

/// Same schema as `group_type_fixture`, but with `grp`'s `START_GROUP`
/// tag encoded with one overhang byte (non-minimal varint: `0xAB, 0x00`
/// instead of the minimal `0x2B`) — exercises the `tag_ohb: 1` anomaly
/// modifier (spec 0122 Test Plan item 2, 3rd bullet).
pub(super) fn group_type_fixture_with_tag_ohb() -> (App, usize) {
    group_type_fixture_with_blob(&[0xABu8, 0x00, 0x08, 0x05, 0x2Cu8])
}

pub(super) fn group_type_fixture_with_blob(blob: &[u8]) -> (App, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let my_group_desc = DescriptorProto {
        name: Some("MyGroup".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let new_group_desc = DescriptorProto {
        name: Some("NewGroup".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("value".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer2".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("grp".to_string()),
            number: Some(5),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Group as i32),
            type_name: Some(".test.MyGroup".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_group_type.proto".to_string()),
        package: Some("test".to_string()),
        syntax: Some("proto2".to_string()),
        message_type: vec![outer_desc, my_group_desc, new_group_desc],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-group-type-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let decoded = decode(blob, &mut ctx, RootType::Named("test.Outer2"), 2).unwrap();
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

    let grp_idx =
        node_with_type(&app, "test.MyGroup").expect("tree must contain the MyGroup submessage");
    (app, grp_idx)
}

/// Builds the shared `Container { extensions: TestMessageSet { Item {
/// type_id: 100, message: ExtPayload { label: "hi" } } } }` fixture
/// used by both the auto-expansion test and the toggle/reactivate
/// regression test below.
pub(super) fn message_set_fixture() -> App {
    use prost::Message as _;
    use prost_types::descriptor_proto::ExtensionRange;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let message_set_msg = DescriptorProto {
        name: Some("TestMessageSet".to_string()),
        options: Some(MessageOptions {
            message_set_wire_format: Some(true),
            ..Default::default()
        }),
        extension_range: vec![ExtensionRange {
            start: Some(1),
            end: Some(536870912),
            ..Default::default()
        }],
        ..Default::default()
    };
    let ext_payload_msg = DescriptorProto {
        name: Some("ExtPayload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let extension_field = FieldDescriptorProto {
        name: Some("ext_payload".to_string()),
        number: Some(100),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".ms_test.ExtPayload".to_string()),
        extendee: Some(".ms_test.TestMessageSet".to_string()),
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("extensions".to_string()),
            number: Some(2),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".ms_test.TestMessageSet".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("ms_test.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("ms_test".to_string()),
        message_type: vec![message_set_msg, ext_payload_msg, container_msg],
        extension: vec![extension_field],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // Unique per call (this fixture is shared by several tests that
    // may run concurrently) to avoid one test's cleanup racing
    // another's read of the same path.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!(
        "protolens-tui-message-set-expand-descriptor-{n}.pb"
    ));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Container { extensions: TestMessageSet {
    //   Item { type_id: 100, message: ExtPayload { label: "hi" } }
    // } }.
    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut blob = vec![0x12u8, item_bytes.len() as u8];
    blob.extend_from_slice(&item_bytes);

    let decoded = decode(&blob, &mut ctx, RootType::Named("ms_test.Container"), 2).unwrap();
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
    app
}

/// `acme.Level1 { Level2 l2 { Level3 l3 { google.protobuf.Any payload
/// } } }`, the `Any` carrying `acme.Payload { label: "hello" }` — the
/// same `Any` shape the inline fixtures in this crate's tests already
/// use, but buried under two plain message levels that carry no
/// override of their own.
///
/// Spec 0183 needs exactly that burial. Once `is_message` leaves the
/// recursion gate, a plain message is descended into only if something
/// beneath it was marked, so `Level2` and `Level3` are the ancestors
/// the seed scan's upward marking walk has to set bits on. A fixture
/// with the `Any` directly under the root would still pass with the
/// upward walk missing entirely, and would prove nothing.
pub(super) fn nested_any_fixture() -> App {
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

    // A single-field message wrapping `type_name` at field 1.
    let wrapper = |name: &str, type_name: &str, field_name: &str| DescriptorProto {
        name: Some(name.to_string()),
        field: vec![FieldDescriptorProto {
            name: Some(field_name.to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(type_name.to_string()),
            ..Default::default()
        }],
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
    let acme_file = FileDescriptorProto {
        name: Some("acme_nested_any.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![
            payload_msg,
            wrapper("Level3", ".google.protobuf.Any", "payload"),
            wrapper("Level2", ".acme.Level3", "l3"),
            wrapper("Level1", ".acme.Level2", "l2"),
        ],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_file, acme_file],
    };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-nested-any-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let mut blob = vec![0x0au8, b"hello".len() as u8];
    blob.extend_from_slice(b"hello");
    let type_url = b"type.googleapis.com/acme.Payload";
    let mut any_bytes = vec![0x0au8, type_url.len() as u8];
    any_bytes.extend_from_slice(type_url);
    any_bytes.push(0x12);
    any_bytes.push(blob.len() as u8);
    any_bytes.append(&mut blob);
    // Three LEN wrappers, all field 1, all short enough for a one-byte
    // length varint: `Level3.payload`, then `Level2.l3`, then
    // `Level1.l2` — the last of which *is* `Level1`'s body, which is
    // what `decode` is handed.
    let mut blob = any_bytes;
    for _ in 0..3 {
        let mut next = vec![0x0au8, blob.len() as u8];
        next.append(&mut blob);
        blob = next;
    }

    let decoded = decode(&blob, &mut ctx, RootType::Named("acme.Level1"), 2).unwrap();
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
    app
}

/// `ms_test.Top { Mid m { Container c { extensions: TestMessageSet {
/// Item { type_id: 100, message: ExtPayload { label: "hi" } } } } } }`
/// — `message_set_fixture`'s document buried under two plain message
/// levels, for the same reason `nested_any_fixture` buries its `Any`.
///
/// MessageSet is the half of spec 0183 S1 that is easy to break
/// silently: tier 1 (the `Item` group wrapper) is deliberately *not* an
/// `is_auto_expand_candidate`, on the recorded grounds that the
/// `is_message` disjunct reaches it. Delete that disjunct without
/// widening the predicate and MessageSet expansion stops, with no
/// panic. The 1.1 MB `FileDescriptorSet` used for profiling contains no
/// MessageSet at all, so only a fixture like this one can catch it.
pub(super) fn nested_message_set_fixture() -> App {
    use prost::Message as _;
    use prost_types::descriptor_proto::ExtensionRange;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        MessageOptions,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let message_set_msg = DescriptorProto {
        name: Some("TestMessageSet".to_string()),
        options: Some(MessageOptions {
            message_set_wire_format: Some(true),
            ..Default::default()
        }),
        extension_range: vec![ExtensionRange {
            start: Some(1),
            end: Some(536870912),
            ..Default::default()
        }],
        ..Default::default()
    };
    let ext_payload_msg = DescriptorProto {
        name: Some("ExtPayload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let extension_field = FieldDescriptorProto {
        name: Some("ext_payload".to_string()),
        number: Some(100),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".ms_test.ExtPayload".to_string()),
        extendee: Some(".ms_test.TestMessageSet".to_string()),
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("extensions".to_string()),
            number: Some(2),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".ms_test.TestMessageSet".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let wrapper = |name: &str, type_name: &str, field_name: &str| DescriptorProto {
        name: Some(name.to_string()),
        field: vec![FieldDescriptorProto {
            name: Some(field_name.to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(type_name.to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("ms_test_nested.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("ms_test".to_string()),
        message_type: vec![
            message_set_msg,
            ext_payload_msg,
            container_msg,
            wrapper("Mid", ".ms_test.Container", "c"),
            wrapper("Top", ".ms_test.Mid", "m"),
        ],
        extension: vec![extension_field],
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path = std::env::temp_dir().join(format!(
        "protolens-tui-nested-message-set-descriptor-{n}.pb"
    ));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    let ext_payload_bytes = [0x0au8, 0x02, b'h', b'i'];
    let mut item_bytes = vec![0x0bu8, 0x10, 100u8];
    item_bytes.push(0x1a);
    item_bytes.push(ext_payload_bytes.len() as u8);
    item_bytes.extend_from_slice(&ext_payload_bytes);
    item_bytes.push(0x0c); // END_GROUP
    let mut container_bytes = vec![0x12u8, item_bytes.len() as u8];
    container_bytes.extend_from_slice(&item_bytes);
    // `Mid { c: Container }` — and then `Top`'s own body is just
    // `m: Mid`, which is what `decode` is handed.
    let mut mid_bytes = vec![0x0au8, container_bytes.len() as u8];
    mid_bytes.append(&mut container_bytes);
    let mut blob = vec![0x0au8, mid_bytes.len() as u8];
    blob.append(&mut mid_bytes);

    let decoded = decode(&blob, &mut ctx, RootType::Named("ms_test.Top"), 2).unwrap();
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
    app
}

/// `test.Outer` (in `outer.proto`, package `test`) with schema fields
/// covering `resolve_export_fields`'s G6c tiers, plus two live
/// children at an undeclared field number (8) for the repeated/
/// tier-4 cases — spec 0156's Test plan. Returns the root-cursor
/// `App`; callers add their own override entries before calling
/// `resolve_export_fields`/`export_descriptor_bytes`.
///
/// Field layout (all direct children of the root):
/// - `1` (`num`, schema `int32`) — tier 3 primitive.
/// - `2` (`msg_field`, schema message `.other.Msg`, a *different*
///   file, `other.proto`) — tier 3 message + G6d dependency.
/// - `3` (`own_type_field`, schema message `.test.OwnType`, the
///   cursor's *own* file, `outer.proto`) — tier 3 message + G6d
///   "no exclusion" of the cursor's own file.
/// - `4` (`retype_field`, schema message `.test.OwnType`) — left for
///   callers to retype via an active `PathField` override (tier 1).
/// - `6` (`retype_field2`, schema message `.test.OwnType`) — left for
///   callers to retype via an active `FqdnField` override (tier 2).
/// - `7` (`raw_field`, schema message `.test.OwnType`) — left for
///   callers to retype to raw (`target: None`) via an active override.
/// - `8` (undeclared, two live children, `WT_VARINT`) — tier 4
///   primitive guess (`int64`), `LABEL_REPEATED` from the live count.
pub(super) fn export_fields_fixture() -> App {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let msg = DescriptorProto {
        name: Some("Msg".to_string()),
        ..Default::default()
    };
    let other_file = FileDescriptorProto {
        name: Some("other.proto".to_string()),
        package: Some("other".to_string()),
        message_type: vec![msg],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };

    let own_type = DescriptorProto {
        name: Some("OwnType".to_string()),
        ..Default::default()
    };
    let field = |name: &str, number: i32| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Message as i32),
        type_name: Some(".test.OwnType".to_string()),
        ..Default::default()
    };
    let outer = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("num".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("msg_field".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".other.Msg".to_string()),
                ..Default::default()
            },
            field("own_type_field", 3),
            field("retype_field", 4),
            field("retype_field2", 6),
            field("raw_field", 7),
        ],
        ..Default::default()
    };
    let outer_file = FileDescriptorProto {
        name: Some("outer.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![own_type, outer],
        dependency: vec!["other.proto".to_string()],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![other_file, outer_file],
    };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-export-fields-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // field 1 = 5, fields 2/3/4/6/7 = empty submessages, field 8 (twice,
    // undeclared) = varints 1 and 2.
    let blob = vec![
        0x08, 0x05, // 1: num = 5
        0x12, 0x00, // 2: msg_field {}
        0x1A, 0x00, // 3: own_type_field {}
        0x22, 0x00, // 4: retype_field {}
        0x32, 0x00, // 6: retype_field2 {}
        0x3A, 0x00, // 7: raw_field {}
        0x40, 0x01, // 8: 1
        0x40, 0x02, // 8: 2
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
    app
}

/// `test.GroupHolder` (no declared fields), whose single live child is
/// an untyped `WT_START_GROUP` field (9) — `resolve_export_fields`'s
/// tier-4 "no supported guess for a group" error case.
pub(super) fn export_fields_group_error_fixture() -> App {
    use prost::Message as _;
    use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};

    use crate::decode::{decode, DescriptorContext, RootType};

    let holder = DescriptorProto {
        name: Some("GroupHolder".to_string()),
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("group_holder.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![holder],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-group-holder-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // field 9 (undeclared): START_GROUP then END_GROUP.
    let blob = vec![0x4B, 0x4C];
    let decoded = decode(&blob, &mut ctx, RootType::Named("test.GroupHolder"), 2).unwrap();
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
    app
}

/// `Outer { Wrap head = 1; Wrap tail = 2; }`, with `Wrap { Leaf leaf =
/// 1; }`, `Leaf { int32 v = 1; }`, and a `Blob { string leaf = 1; }`
/// that reads the very same bytes as one line instead of three.
///
/// The shape a splice's line-count delta gets lost in, and the one no
/// other fixture here has: a node that can be retyped into a *shorter*
/// rendering, followed by a sibling whose own rendering cannot change —
/// so the override walk prunes it — but which still has an interior
/// that has to move. Returns `(app, head, tail, tail_leaf, tail_v)`.
pub(super) fn pruned_tail_fixture() -> (App, usize, usize, usize, usize) {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext, RootType};

    let leaf_desc = DescriptorProto {
        name: Some("Leaf".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("v".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let wrap_desc = DescriptorProto {
        name: Some("Wrap".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("leaf".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".test.Leaf".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let blob_desc = DescriptorProto {
        name: Some("Blob".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("leaf".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let outer_desc = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("head".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Wrap".to_string()),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("tail".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Wrap".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_pruned_tail.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, wrap_desc, blob_desc, leaf_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let descriptor_path =
        std::env::temp_dir().join(format!("protolens-tui-pruned-tail-descriptor-{n}.pb"));
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // head (field 1, LEN) then tail (field 2, LEN), each holding
    // `Wrap { leaf: Leaf { v: 5 } }` == `0A 02 08 05`.
    let blob = [
        0x0Au8, 0x04, 0x0A, 0x02, 0x08, 0x05, //
        0x12, 0x04, 0x0A, 0x02, 0x08, 0x05,
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

    let root = app.first_node;
    let head = app.tree[root].first_child().expect("Outer has children");
    let tail = app.tree[head]
        .next_sibling()
        .expect("Outer has two children");
    let tail_leaf = app.tree[tail].first_child().expect("tail wraps a Leaf");
    let tail_v = app.tree[tail_leaf].first_child().expect("Leaf holds v");
    (app, head, tail, tail_leaf, tail_v)
}
