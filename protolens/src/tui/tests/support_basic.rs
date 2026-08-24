// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `App` builders with no fixture bytes to speak of: an empty one, a
//! one-message one, and the sibling-leaf generators that navigation and
//! rendering tests count rows against — plus the pan driver those tests
//! share.

use super::super::heat_cue::HEAT_CUE_PREVIEW;
use super::super::pane_scroll::{EDGE_HOLD, EDGE_PUSHES};
use super::super::*;
use super::support_build::app_named;
use prototext_core::helpers::{WT_LEN, WT_VARINT};
use prototext_core::serialize::render_text::{Label, NodeSpan};
use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::score::load::LoadedGraph;

/// Spec 0222 S1's `node_text` for a fixture whose nodes are one flat
/// line each, in slot order.
fn own_line_each(lines: &[String]) -> Vec<Option<Box<str>>> {
    lines.iter().map(|l| Some(Box::from(l.as_str()))).collect()
}

pub(super) fn empty_app() -> App {
    let decoded = Decoded {
        total_lines: 0,
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        // Spec 0323 S2: a hand-built tree writes its own counts, so
        // nothing in it is folded.
        user_folded: FoldSet::default(),
        row_budget: None,
        node_text: Vec::new(),
        tree: Vec::new(),
        root_type: Some("google.protobuf.Empty".to_string()),
        arena: crate::decode::arena_of(&[]),
        blob: Arc::new(Blob::unwrapped(Vec::new())),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    app_named(decoded, DescriptorContext::empty_for_test(), "empty.pb")
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
    // Tag `0x22` = field 4 << 3 | WT_LEN(2), length varint `0x08` = 8,
    // then 8 zero payload bytes — a real, `raw_range`-consistent blob,
    // needed since spec 0132's live preview now splices this node's
    // contents at pane-open time.
    let blob = vec![0x22, 0x08, 0, 0, 0, 0, 0, 0, 0, 0];
    let span = NodeSpan {
        field_number: 4,
        raw_range: 0..10,
        text_range: 0..2,
        level: 0,
        type_fqdn: fqdns.intern("google.protobuf.DescriptorProto"),
        kind: NodeKind::Message,
        packed_record_start: NO_PACKED_RECORD,
        wire_and_label: NodeSpan::pack(WT_LEN as u8, Label::NoSchema),
    };
    // Spec 0216: the shape is the blob's, so the overlay is derived
    // rather than written out. The zero payload bytes decompose into
    // malformed child slots the arena keeps and this interpretation
    // does not show, which is exactly the vacant case.
    let arena = crate::decode::arena_of(&blob);
    let built = crate::decode::build_tree(vec![span], &lines, &arena, &[]);
    let decoded = Decoded {
        total_lines: lines.len(),
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        row_budget: None,
        // Spec 0323 S2: from the same pass that wrote the counts.
        user_folded: built.user_folded,
        node_text: built.node_text,
        tree: built.tree,
        root_type: Some("google.protobuf.FileDescriptorProto".to_string()),
        arena,
        blob: Arc::new(Blob::unwrapped(blob)),
        wrapper_offset: 0,
        root_candidates,
        fqdns,
    };
    app_named(decoded, DescriptorContext::empty_for_test(), "test.pb")
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

/// `n` scalar sibling nodes at the root level (spec 0113 D16:
/// root-level nodes are siblings of each other despite having no
/// `parent`), one line of text each — the minimal fixture for
/// exercising main-pane search (spec 0114 §4, extended from the
/// override pane), which walks the document order.
///
/// Spec 0216: the shape is the blob's, so the fixture states bytes and
/// the arena derives the tree. `n` top-level varint records give `n`
/// slots at level 0, each its own parent, so node `i` is line `i` and
/// every call site's hard-coded index still names the leaf it always
/// named. The blob is deliberately *not* wrapped: wrapping would make
/// the leaves children of a rendered root that owns a line of its own.
pub(super) fn sibling_leaves_app(texts: &[&str]) -> App {
    let lines: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    let n = lines.len();
    let mut blob: Vec<u8> = Vec::new();
    for i in 0..n {
        let mut tag = ((i as u64 + 1) << 3) | u64::from(WT_VARINT);
        while tag >= 0x80 {
            blob.push(tag as u8 | 0x80);
            tag >>= 7;
        }
        blob.push(tag as u8);
        blob.push(0); // the value: varint 0
    }
    let arena = crate::decode::arena_of(&blob);
    assert_eq!(arena.len(), n, "one slot per top-level record");
    let (raw_start, raw_end) = (arena.raw_start(), arena.raw_end());
    let tree: Vec<TreeNode> = (0..n)
        .map(|i| TreeNode {
            span: NodeSpan {
                field_number: i as u32 + 1,
                raw_range: raw_start[i]..raw_end[i],
                text_range: i as u32..i as u32 + 1,
                level: 0,
                type_fqdn: NO_FQDN,
                kind: NodeKind::Varint,
                packed_record_start: NO_PACKED_RECORD,
                wire_and_label: NodeSpan::pack(WT_VARINT as u8, Label::NoSchema),
            },
            lines_total: 1,
            lines_visible: 1,
            rendered_as: NOT_RENDERED,
        })
        .collect();
    let decoded = Decoded {
        total_lines: lines.len(),
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        // Spec 0323 S2: a hand-built tree writes its own counts, so
        // nothing in it is folded.
        user_folded: FoldSet::default(),
        row_budget: None,
        node_text: own_line_each(&lines),
        tree,
        root_type: Some("google.protobuf.FileDescriptorProto".to_string()),
        arena,
        blob: Arc::new(Blob::unwrapped(blob)),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    app_named(decoded, DescriptorContext::empty_for_test(), "test.pb")
}

/// `n` document-order sibling scalar (`WT_VARINT`) fields, one line
/// each, each backed by a real 2-byte tag+value encoding in `blob` —
/// `sibling_leaves_app` at the scale spec 0191's budget tests need,
/// which run `n` into the thousands.
///
/// Field numbers cycle through 1..=15 rather than counting up, so the
/// tag always fits the single byte written here and a wider tag would
/// need a real varint. Nodes stay distinct because each has its own
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
                    kind: NodeKind::Varint,
                    packed_record_start: NO_PACKED_RECORD,
                    wire_and_label: NodeSpan::pack(WT_VARINT as u8, Label::NoSchema),
                },
                lines_total: 1,
                lines_visible: 1,
                rendered_as: NOT_RENDERED,
            }
        })
        .collect();
    let decoded = Decoded {
        total_lines: lines.len(),
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        // Spec 0323 S2: a hand-built tree writes its own counts, so
        // nothing in it is folded.
        user_folded: FoldSet::default(),
        row_budget: None,
        node_text: own_line_each(&lines),
        tree,
        root_type: Some("google.protobuf.FileDescriptorProto".to_string()),
        arena: crate::decode::arena_of(&blob),
        blob: Arc::new(Blob::unwrapped(blob)),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    app_named(decoded, DescriptorContext::empty_for_test(), "test.pb")
}

/// One of the three panes [`pan_to_the_bound`] can lean on. Each names
/// a viewport, the `PAN_STEP` pan that drives it and the spec 0286 wall
/// in front of it — which is all that differs between them.
#[derive(Clone, Copy, Debug)]
pub(super) enum Pannable {
    Main,
    Override,
    Manage,
}

impl Pannable {
    fn top(self, app: &App) -> isize {
        match self {
            Self::Main => app.scroll_top(),
            Self::Override => app.override_scroll.top(&FLAT_ROWS),
            Self::Manage => app.manage_scroll.top(&FLAT_ROWS),
        }
    }

    fn pan(self, app: &mut App, down: bool) {
        match self {
            Self::Main if down => app.pan_vertical_down(),
            Self::Main => app.pan_vertical_up(),
            Self::Override => app.override_pan_vertical(PAN_STEP, !down),
            Self::Manage => app.manage_pan_vertical(PAN_STEP, !down),
        }
    }

    fn backdate(self, app: &mut App) {
        match self {
            Self::Main => app.scroll_resistance.backdate(EDGE_HOLD),
            Self::Override => app.override_resistance.backdate(EDGE_HOLD),
            Self::Manage => app.manage_resistance.backdate(EDGE_HOLD),
        }
    }
}

/// Pans `pane` until it will not move again, leaning on spec 0286's wall
/// as often as it stands back up.
///
/// Spec 0244's over-pan lies *past* that wall, so a test about the
/// over-pan bounds has to lean on it rather than expect to arrive in one
/// step. Each refused pan is back-dated by the hold rather than slept
/// through, which is what keeps this instant. Terminates: once outside a
/// natural bound a pan is free again and counts no push, so a run of
/// refusals there is a run of real refusals.
pub(super) fn pan_to_the_bound(app: &mut App, pane: Pannable, down: bool) {
    let mut refused = 0;
    while refused <= EDGE_PUSHES {
        let before = pane.top(app);
        pane.pan(app, down);
        if pane.top(app) != before {
            refused = 0;
            continue;
        }
        refused += 1;
        // Refused by either the wall — which holding against it gets
        // through — or spec 0244's own bound, where nothing is pushing
        // and this is a no-op.
        pane.backdate(app);
    }
}
