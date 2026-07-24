// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use std::thread;

use crate::override_pane::OverrideCollection;

use super::super::heat_worker::{HeatWorkerHandle, RangeHeatEntry};
use super::super::*;
use super::support::*;

/// Spec 0114 §1/§2: `t` opens the override pane for a message-shaped
/// cursor node and moves focus there; a second `t` (from either
/// pane's focus) closes it again.
#[test]
fn t_opens_and_closes_the_override_pane_on_a_message_node() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(0));
    assert!(app.override_focus);

    // `t` from override-pane focus closes it too.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert!(!app.override_focus);
}

/// `q` closes the override pane (not just blurs focus, unlike
/// `Tab`).
#[test]
fn override_pane_q_closes_pane() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(0));
    assert!(app.override_focus);

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert!(!app.override_focus);
}

/// Spec 0134 G1: the override selection pane no longer has a `z`/`Z`
/// kind-rotation key — pressing either is a no-op (no panic, pane stays
/// open, no kind rotation). `Enter` defaults to a `PathField`-kind
/// origin (feedback, 2026-07-21), falling back to `Path` at the
/// wrapper root — this fixture's single node has no parent, so it
/// exercises that fallback and still asserts `Path`.
/// (Spec 0147 G5: every keypress, `z`/`Z` included, now unconditionally
/// dismisses a stale `self.message` — so unlike before spec 0147, `z`/`Z`
/// are no longer asserted to leave `self.message` untouched.)
#[test]
fn override_pane_z_is_a_noop() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);

    let sort_before = app.override_sort;
    let highlight_before = app.override_highlight;
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, sort_before);
    assert_eq!(app.override_highlight, highlight_before);
    assert!(app.override_focus, "pane must stay open");
    app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, sort_before);
    assert_eq!(app.override_highlight, highlight_before);
    assert!(app.override_focus, "pane must stay open");

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let entry = app
        .overrides
        .entries()
        .iter()
        .find(|e| e.active)
        .expect("Enter must create an active entry");
    assert!(matches!(entry.origin, OverrideOrigin::Path { .. }));
}

/// Spec 0114 §1.2: `t` also opens the pane on a message/group node
/// whose type wasn't resolved by the schema (`type_fqdn: None`, as
/// produced by the unknown-LEN-field probe cascade) — this is the bug
/// reported during interactive testing of Task #17, where every node
/// looked scalar-shaped to `type_fqdn.is_none()` under a schema
/// declaring no fields for the target type.
#[test]
fn t_opens_the_override_pane_on_an_unresolved_message_node() {
    let lines: Vec<String> = vec!["1 {".to_string(), "}".to_string()];
    let node = TreeNode {
        span: NodeSpan {
            field_number: 1,
            raw_range: 0..2,
            text_range: 0..2,
            level: 0,
            type_fqdn: None,
            is_message: true,
            packed_record_start: None,
            wire_type: WT_LEN,
            natural_annotation: None,
        },
        parent: None,
        first_child: None,
        last_child: None,
        next_sibling: None,
        prev_sibling: None,
        doc_next: None,
        doc_prev: None,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines,
        tree: vec![node],
        root_type: "google.protobuf.Empty".to_string(),
        // Tag `0x0A` = field 1 << 3 | WT_LEN(2), length varint `0x00`
        // = 0, zero payload bytes — a real, `raw_range`-consistent
        // blob, needed since spec 0132's live preview now splices
        // this node's contents at pane-open time.
        blob: vec![0x0A, 0x00],
        wrapper_offset: 0,
        style_hints: vec![Vec::new(); 2],
        root_type_deferred: false,
    };
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(0));
    assert!(app.override_focus);
}

/// Spec 0135 §G3 (test plan item 12): `can_override`/`t` now accept a
/// plain `WT_VARINT` scalar node too — a wire-compatible primitive
/// override target (`:type-as sint32`, etc.) is available for it, so it
/// is no longer treated as ineligible the way it was pre-0135.
#[test]
fn t_opens_the_override_pane_on_a_varint_scalar_field() {
    let lines: Vec<String> = vec!["value: 1".to_string()];
    let node = TreeNode {
        span: NodeSpan {
            field_number: 1,
            raw_range: 0..2,
            text_range: 0..1,
            level: 0,
            type_fqdn: None,
            is_message: false,
            packed_record_start: None,
            wire_type: WT_VARINT,
            natural_annotation: None,
        },
        parent: None,
        first_child: None,
        last_child: None,
        next_sibling: None,
        prev_sibling: None,
        doc_next: None,
        doc_prev: None,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines,
        tree: vec![node],
        root_type: "test.Scalar".to_string(),
        // Tag `0x08` = field 1 << 3 | WT_VARINT(0), value varint `0x01`
        // — a real, `raw_range`-consistent blob, needed since spec 0132's
        // live preview now splices this node's contents at pane-open
        // time.
        blob: vec![0x08, 0x01],
        wrapper_offset: 0,
        style_hints: vec![Vec::new()],
        root_type_deferred: false,
    };
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(0));
    assert!(app.override_focus);
}

/// Regression test (spec 0135 follow-up, 2026-07-17): pressing `t` on
/// a plain primitive (non-message) field, then immediately `Esc`
/// (cancelling, no navigation in between), must leave the main-pane
/// rendering exactly as it was before `t` was pressed. Root-caused to
/// `natural_type` returning `None` for every non-message field kind,
/// which `resettle_node`'s no-active-override fallback treated as
/// "render raw" rather than "this field's own natural primitive
/// type" — reachable only once spec 0135 §G3 widened `can_override`
/// to plain scalar leaves in the first place.
#[test]
fn esc_after_t_on_a_primitive_field_restores_its_original_rendering() {
    let (mut app, _, id_idx) = type_as_fixture();
    app.cursor = id_idx;
    let line_idx = app.tree[id_idx].span.text_range.start;
    let original_line = app.lines[line_idx].clone();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(id_idx));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(
        app.lines[line_idx], original_line,
        "the field's rendering must be restored exactly, not left raw"
    );
}

/// Spec 0139 Step B.5 (2026-07-18 feedback): `t` on an enum-typed
/// scalar field with no active/matching override entry must open in
/// `Lexicographic` mode (no enum candidate can ever appear in
/// `Inferred` mode's message-shaped scoring) with the highlight
/// already on the field's own natural enum type — not the `None`
/// sentinel row.
#[test]
fn t_opens_on_an_enum_field_highlighting_its_own_natural_type() {
    let (mut app, durability_idx) = enum_field_fixture();
    app.cursor = durability_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(
        app.override_candidates[app.override_highlight].0,
        "test.Durability"
    );
}

/// 2026-07-19 feedback item 6: `t` on a schema-typed primitive field
/// with no active or inactive override always opens `Lexicographic`
/// mode, highlighted on the row matching the field's own natural
/// primitive keyword — never `Inferred` mode, which is meaningless
/// for a scalar (no primitive keyword is ever a member of the
/// `Inferred` candidate list).
#[test]
fn t_opens_on_a_primitive_field_highlighting_its_own_natural_type() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.cursor = id_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(app.override_candidates[app.override_highlight].0, "int32");
}

/// 2026-07-20 feedback: `t` on a message-typed field with no active
/// or inactive override must still open highlighted on its own
/// schema-declared (natural) type — the type already shown for the
/// node in the main pane — not the top `Inferred`-scored guess.
/// Previously `Kind::Message` was deliberately excluded from Step
/// B.5 on the theory that a message field's schema is "unknown by
/// nature"; but `natural_type` already returns `None` in exactly
/// that unresolved case (`parent_field` fails), so when it *does*
/// resolve — as here — its declared type is exactly as fixed and
/// known as an enum's or a primitive's.
#[test]
fn t_opens_on_a_message_field_highlighting_its_own_natural_type() {
    let (mut app, inner_idx) = empty_message_fixture();
    app.cursor = inner_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(
        app.override_candidates[app.override_highlight].0,
        "test.Inner"
    );
}

/// Regression test (2026-07-18 feedback): pressing `t` on an
/// enum-typed scalar field, then immediately `Esc`, must leave the
/// main-pane rendering exactly as it was before `t` was pressed —
/// same bug class as `esc_after_t_on_a_primitive_field_...` above,
/// but for `Kind::Enum`, which `natural_type` excluded until this
/// fix (`resettle_node`'s no-active-override fallback demoted the
/// field to a raw record dump, permanently, since no other render
/// pass ever revisits a plain scalar leaf).
#[test]
fn esc_after_t_on_an_enum_field_restores_its_original_rendering() {
    let (mut app, durability_idx) = enum_field_fixture();
    app.cursor = durability_idx;
    let line_idx = app.tree[durability_idx].span.text_range.start;
    let original_line = app.lines[line_idx].clone();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(durability_idx));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(
        app.lines[line_idx], original_line,
        "the enum field's rendering must be restored exactly, not left raw"
    );
}

/// 2026-07-14 feedback: `t` must not refuse a plain string/bytes
/// field just because it isn't schema-typed as a message — it's
/// still `WT_LEN`-wire and may in practice carry an embedded
/// submessage the schema doesn't know about, so the user should be
/// free to attempt reinterpreting it.
#[test]
fn t_opens_the_override_pane_on_a_length_delimited_scalar_field() {
    let lines: Vec<String> = vec!["value: \"hi\"".to_string()];
    let node = TreeNode {
        span: NodeSpan {
            field_number: 1,
            raw_range: 0..4,
            text_range: 0..1,
            level: 0,
            type_fqdn: None,
            is_message: false,
            packed_record_start: None,
            wire_type: WT_LEN,
            natural_annotation: None,
        },
        parent: None,
        first_child: None,
        last_child: None,
        next_sibling: None,
        prev_sibling: None,
        doc_next: None,
        doc_prev: None,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines,
        tree: vec![node],
        root_type: "test.Scalar".to_string(),
        blob: vec![0x0A, 0x02, b'h', b'i'],
        wrapper_offset: 0,
        // One entry per `lines` (spec 0132's live preview now
        // splices this node at pane-open time, which requires
        // `line_styles` to stay index-aligned with `lines`).
        style_hints: vec![Vec::new(); 1],
        root_type_deferred: false,
    };
    let mut app = App::new(
        decoded,
        "test.pb",
        PathBuf::from("test.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, Some(0));
}

/// Spec 0114 §2: `t` refuses to open the pane below the minimum
/// terminal width.
#[test]
fn t_refuses_below_the_minimum_terminal_width() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = MIN_OVERRIDE_WIDTH - 1;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert!(app.message.contains("too narrow"));
}

/// Spec 0114 §3.2: `i` toggles between the two sort modes. (Which mode
/// `t` opens the pane in initially is spec 0139's smart-open logic,
/// covered by its own tests below — `message_node_app` has no scoring
/// graph, so `t` opens directly in `Lexicographic` mode, spec 0139
/// G3.)
#[test]
fn override_i_toggles_the_sort_mode() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Inferred);

    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
}

/// Spec 0139 Step A + mode-selection rule: an active override whose
/// type is a primitive keyword can never be present in the inferred
/// candidate list (by construction — `inferred_candidates` only ever
/// produces message/enum FQDNs), so `t` must fall through to opening
/// in `Lexicographic` mode with the highlight on that keyword's row.
#[test]
fn t_opens_on_active_primitive_override_in_lexicographic_mode() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    let origin = app.override_origin_for_kind(app.cursor).unwrap();
    app.overrides.activate(origin, Some("fixed32".to_string()));

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(app.override_candidates[app.override_highlight].0, "fixed32");
}

/// Spec 0139 Step A + mode-selection rule: an active override typed
/// raw (`Option::None`) opens in `Lexicographic` mode with the
/// highlight on the `None` sentinel row.
#[test]
fn t_opens_on_active_raw_override_on_the_none_sentinel_row() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    let origin = app.override_origin_for_kind(app.cursor).unwrap();
    app.overrides.activate(origin, None);

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(
        app.override_candidates[app.override_highlight].0,
        "protolens_internal.None"
    );
}

/// Spec 0139 Step B: no override is active for the cursor node, but
/// the management list holds an inactive entry whose origin exactly
/// matches it — `t` picks up that entry's type and applies the same
/// mode-selection rule as an active override would (spec 0139 §G1
/// "apply the rules of the preceding point").
#[test]
fn t_opens_on_first_inactive_matching_entry_when_none_is_active() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    // Drop the seeded root entry first — it shares this fixture's only
    // node's origin and would otherwise sort ahead of (or instead of)
    // the entry this test is specifically about.
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    let origin = app.override_origin_for_kind(app.cursor).unwrap();
    app.overrides
        .activate(origin.clone(), Some("int32".to_string()));
    let idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin && e.r#type.as_deref() == Some("int32"))
        .unwrap();
    app.overrides.toggle_active(idx);
    assert!(!app.overrides.entries()[idx].active, "must be inactive");

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(app.override_candidates[app.override_highlight].0, "int32");
}

/// Spec 0139 Steps C/D + G3: with neither an active nor an
/// applicable-inactive override, and no scoring graph loaded at all
/// (`message_node_app`'s fixture), `t` falls straight through to
/// `Lexicographic` mode (highlight on the `None` sentinel row) without
/// ever surfacing the "no scoring graph available" message — the
/// fallback already did what that message would have suggested.
#[test]
fn t_falls_back_to_lexicographic_silently_when_no_graph_and_no_match() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.message.clear();
    // `message_node_app`'s single node is also the seeded root
    // override's own target — remove it so neither Step A nor Step B
    // find a match, exercising Steps C/D in isolation.
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    assert_eq!(
        app.override_candidates[app.override_highlight].0,
        "protolens_internal.None"
    );
    assert!(
        !app.message.contains("no scoring graph"),
        "message must be suppressed on this auto-fallback path: {}",
        app.message
    );
}

/// Spec 0114 §3.2/spec 0137 §G4: `j`/`k` move the highlight, clamped to
/// `0..=candidates.len() - 1` — direct indexing, no pinned raw row.
#[test]
fn override_highlight_movement_clamps_at_both_ends() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![("a.B".to_string(), None), ("a.C".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1);
}

/// Spec 0114 §4/spec 0137 §G4: `/` searches forward, `?` searches
/// backward, `n` repeats the last search — wrapping around — over
/// `override_candidates` directly, no pinned raw row excluded.
#[test]
fn override_search_forward_backward_and_repeat_with_n() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.Alpha".to_string(), None),
        ("pkg.Beta".to_string(), None),
        ("pkg.Gamma".to_string(), None),
        ("pkg.Beta2".to_string(), None),
    ];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.override_highlight, 1); // pkg.Beta

    // `n` repeats forward, wrapping to the next match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 3); // pkg.Beta2

    // Wraps back around to the first match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta

    // `?` searches backward from the current highlight (pkg.Beta,
    // index 1) — skips itself, wraps to pkg.Beta2 (index 3).
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 3); // pkg.Beta2

    // No match leaves the highlight unchanged and sets a message.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "nope".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 3);
    assert!(app.message.contains("not found"));
}

/// Spec 0132 §G2, extended (2026-07-17 feedback): a search-jump
/// (`/`/`?`/`n`/`p`) live-previews the reached candidate in the main
/// pane, same as arrow-key movement — not just silently moving the
/// highlight. Exercised indirectly: the fake candidate FQDNs used here
/// don't resolve against an empty descriptor context, so a preview
/// attempt surfaces as a "cannot preview override" message; before this
/// fix, a search-jump left the message untouched.
#[test]
fn override_search_jump_previews_the_reached_candidate() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.Alpha".to_string(), None),
        ("pkg.Beta".to_string(), None),
    ];
    app.override_highlight = 0;
    app.message.clear();

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta
    assert!(
        app.message.contains("cannot preview override"),
        "search-jump must preview the reached candidate: {}",
        app.message
    );
}

/// `p` repeats the last search in the opposite direction (vim's `N`
/// counterpart to `n`).
#[test]
fn override_search_repeat_with_p_reverses_direction() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.Alpha".to_string(), None),
        ("pkg.Beta".to_string(), None),
        ("pkg.Gamma".to_string(), None),
        ("pkg.Beta2".to_string(), None),
    ];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta

    // `p` repeats backward (opposite of the forward `/` that set this
    // pattern), wrapping to pkg.Beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 3); // pkg.Beta2

    // A second `p` continues backward, wrapping to pkg.Beta.
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta
}

/// Spec 0114 §4 (vim convention): confirming `/` or `?` with an empty
/// pattern re-uses the last active search pattern, searching in
/// whichever direction the key that opened this prompt requested —
/// which may differ from the direction the pattern was originally
/// searched in.
#[test]
fn override_search_with_no_argument_reuses_the_active_pattern() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.Alpha".to_string(), None),
        ("pkg.Beta".to_string(), None),
        ("pkg.Gamma".to_string(), None),
        ("pkg.Beta2".to_string(), None),
    ];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta

    // `/<Enter>` with no typed pattern re-uses "beta", searching
    // forward from the current highlight — wraps to pkg.Beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 3); // pkg.Beta2

    // `?<Enter>` with no typed pattern re-uses "beta" too, but now
    // searches backward from the current highlight.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.override_highlight, 1); // pkg.Beta
}

/// Spec 0147 G2: the override-select pane's local statusline reads
/// "inferred types" in `Inferred` mode and "all types" in
/// `Lexicographic` mode.
#[test]
fn override_statusline_wording_differs_by_sort_mode() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_target.is_some());

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.override_sort = SortMode::Inferred;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.side_area.y + app.side_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains("inferred types"),
        "Inferred mode must read \"inferred types\": {row_text:?}"
    );

    app.override_sort = SortMode::Lexicographic;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains("all types"),
        "Lexicographic mode must read \"all types\": {row_text:?}"
    );
}

/// Spec 0147 G5: a message set while the override pane has focus is
/// cleared by the *next* keypress handled by `handle_override_key`,
/// not just by a keypress that reaches main-pane handling.
#[test]
fn message_is_dismissed_by_the_next_key_in_the_override_pane() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);

    app.message = "stale notice".to_string();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(
        app.message.is_empty(),
        "the next override-pane key must dismiss a stale message: {}",
        app.message
    );
}

/// Spec 0114 §4: `Esc` cancels an in-progress search without moving the
/// highlight, and `Backspace` on an empty buffer also cancels it.
#[test]
fn override_search_esc_and_empty_backspace_cancel() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.override_candidates = vec![("pkg.Alpha".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.override_highlight, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
}

/// Spec 0114 §4, extended to the main pane: `/`/`?` open a search
/// prompt on the shared command-line row, `n` repeats the last search
/// in the same direction, and matches wrap around the document.
#[test]
fn main_pane_search_forward_backward_and_repeat_with_n() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;
    assert_eq!(app.cursor, 0); // alpha

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert_eq!(app.command_buffer.as_deref(), Some(""));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.cursor, 1); // beta

    // `n` repeats forward, wrapping to the next match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // Wraps back around to the first match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `?` searches backward from the cursor (beta) — skips itself,
    // wraps to beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // No match leaves the cursor unchanged and sets a message.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "nope".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3);
    assert!(app.message.contains("not found"));
}

/// `p` repeats the last main-pane search in the opposite direction.
#[test]
fn main_pane_search_repeat_with_p_reverses_direction() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `p` repeats backward, wrapping to beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // A second `p` continues backward, wrapping to beta.
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta
}

/// Spec 0114 §4 (vim convention), extended to the main pane:
/// confirming `/` or `?` with an empty pattern re-uses the last
/// active search pattern, searching in the newly chosen direction.
#[test]
fn main_pane_search_with_no_argument_reuses_the_active_pattern() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `/<Enter>` with no typed pattern re-uses "beta", forward.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // `?<Enter>` with no typed pattern re-uses "beta" too, backward.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta
}

/// Spec 0114 §4, extended to the main pane: `Esc` cancels an
/// in-progress search without moving the cursor, and `Backspace` on an
/// empty buffer also cancels it.
#[test]
fn main_pane_search_esc_and_empty_backspace_cancel() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.cursor, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
}

/// Spec 0114 §4's main-pane search directive: "search in main pane
/// requires main pane to be in focus" — while the override pane has
/// focus, `/`/`?`/`n` share the same `command_buffer` as main-pane
/// search (spec-0133-adjacent rework), but `Enter` dispatches to the
/// override pane's own `jump_to_override_match`, not the main pane's
/// `jump_to_match` — the main-pane cursor never moves.
#[test]
fn main_pane_search_keys_are_inert_while_override_pane_has_focus() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    let cursor_before = app.cursor;

    app.override_candidates = vec![("pkg.Alpha".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    for c in "alpha".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.override_highlight, 0); // pkg.Alpha
    assert_eq!(app.cursor, cursor_before);
}

/// Spec 0114 §4's main-pane search directive: search matches against
/// the *current* rendered text (`self.lines`), so a range whose type
/// has been overridden is matched post-override, not against the
/// original rendering — there is no separate "original text" cache to
/// special-case.
#[test]
fn main_pane_search_matches_the_current_not_original_rendering() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    // Simulate an already-applied override splice (spec 0114 §5):
    // node 1's rendered line no longer contains "beta" at all.
    app.lines[1] = "pkg.Overridden { x: 1 }".to_string();

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 0); // unchanged — "beta" no longer present
    assert!(app.message.contains("not found"));

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "overridden".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // matches the overridden text
}

/// `Esc` closes the override pane regardless of which pane currently
/// has focus — same "works regardless of focus" treatment as `t`
/// (spec 0114 §8's key-bindings table).
#[test]
fn esc_closes_the_override_pane_from_either_focus() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!app.override_focus);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
}

/// Spec 0114 §5: `Enter` in the override pane applies the highlighted
/// row (the pinned raw entry, or a ranked candidate) and closes the
/// pane on success.
#[test]
fn enter_key_applies_override_and_closes_pane() {
    use prost::Message as _;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    use crate::decode::{decode, DescriptorContext};

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
        name: Some("test_enter_override.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![outer_desc, inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let descriptor_path = std::env::temp_dir().join("protolens-tui-enter-override-descriptor.pb");
    std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
    let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
    std::fs::remove_file(&descriptor_path).unwrap();

    // Outer { inner: Inner { id: 5 } }.
    let blob = [0x0Au8, 0x02, 0x08, 0x05];
    let decoded = decode(&blob, &mut ctx, Some("test.Outer"), 2, false).unwrap();
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

    let inner_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
        .expect("tree must contain the Inner submessage");
    app.cursor = inner_idx;

    // Spec 0137 §G4: inferred mode has no raw/`None` row at all, so
    // reaching raw via the pane requires alphabetic mode, where index
    // `0` is always the `None` sentinel. `Enter` there clears the
    // type and closes the pane.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_target.is_some());
    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();
    app.message.clear();
    app.override_highlight = 0;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_target.is_none(), "pane must close on success");
    assert_eq!(app.tree[inner_idx].span.type_fqdn, None);
    assert!(app.message.is_empty(), "no error expected: {}", app.message);
    // Spec 0119 G3: `Enter` lands in the management pane instead of
    // just closing outright.
    assert!(app.manage_open, "must open the management pane (G3)");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.manage_open);

    // A ranked candidate row: re-open, switch to lexicographic sort
    // (no scoring graph in this fixture), and select the first real
    // message FQDN (spec 0137 §G4: index `0` there is `None`, `1..16`
    // are the primitive keywords, so the first message FQDN comes
    // after those).
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();
    assert!(!app.override_candidates.is_empty());
    let chosen = app.all_type_fqdns[0].clone();
    let row = app
        .override_candidates
        .iter()
        .position(|(f, _)| *f == chosen)
        .expect("chosen FQDN must be a candidate");
    app.override_highlight = row;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_target.is_none());
    assert_eq!(
        app.tree[inner_idx].span.type_fqdn.as_deref(),
        Some(chosen.as_str())
    );
}

/// Spec 0152 G7 test plan: `recompute_override_candidates` in
/// `Inferred` mode, on a `heat_lookup` miss for the pane's first
/// page, sets `override_candidates_pending` and pushes exactly one
/// request, leaving `override_candidates` empty until the cache
/// answers; a range whose window the cache already covers *before*
/// `recompute_override_candidates` is ever called is instead applied
/// immediately, with no pending flag and no request pushed.
#[test]
fn recompute_override_candidates_pushes_pending_on_miss_and_applies_pre_populated_hit() {
    // Miss: no cache entry yet.
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.override_target = Some(0);
    app.override_sort = SortMode::Inferred;
    app.override_list_height = 4;

    app.recompute_override_candidates();
    assert!(app.override_candidates_pending);
    assert!(app.override_candidates.is_empty());
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);
    assert!(app.message.contains("Scoring candidates"));

    // Hit: pre-populate the cache before ever calling `recompute_
    // override_candidates` — a fresh `App` so `active_override_range`
    // hasn't already latched onto this range (the session-reuse fast
    // path would otherwise skip the cache lookup entirely).
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.override_target = Some(0);
    app.override_sort = SortMode::Inferred;
    app.override_list_height = 4;
    let idx = 0;
    let range = extract::message_payload_range(
        &app.blob,
        &app.tree[idx].span.raw_range,
        app.tree[idx].span.packed_record_start,
    );
    app.heat_caches.lock().unwrap().by_range.insert(
        range.start,
        RangeHeatEntry {
            best_score: Some(9),
            best_count: 1,
            top_n: vec![("pkg.Type".to_string(), 9); 4],
        },
    );

    app.recompute_override_candidates();
    assert!(!app.override_candidates_pending);
    assert_eq!(app.override_candidates.len(), 4);
    assert_eq!(
        app.heat_worker.as_ref().unwrap().queue_len(),
        0,
        "a hit must not push a request"
    );
}

/// Spec 0152 G7 test plan (2026-07-20 redesign — a one-shot full-list
/// fetch, `[0, usize::MAX)`, replaces the previous one-page-at-a-time
/// growth): `upgrade_active_override_to_complete` on a miss sets
/// `override_complete_pending` and pushes a request;
/// `poll_pending_override_work` re-checking while the shared cache's
/// `complete` slot holds a *different* range must leave the pending
/// flag set and `override_inferred_raw` untouched (the mismatch
/// guard). Populating `by_range` alone — even for the right range —
/// can never satisfy a `[0, usize::MAX)` request on its own (`top_n`
/// can never hold `usize::MAX` entries), so the flag must stay set
/// even then; only a `complete` hit for the exact range applies the
/// full list wholesale and clears the flag.
#[test]
fn upgrade_active_override_to_complete_pushes_pending_and_respects_the_mismatch_guard() {
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.override_target = Some(0);
    app.override_sort = SortMode::Inferred;
    app.override_list_height = 4;
    app.override_inferred_raw = vec![("pkg.A".to_string(), 5), ("pkg.B".to_string(), 4)];
    app.override_candidates = app
        .override_inferred_raw
        .iter()
        .map(|(f, s)| (f.clone(), Some(*s)))
        .collect();

    let idx = 0;
    let range = extract::message_payload_range(
        &app.blob,
        &app.tree[idx].span.raw_range,
        app.tree[idx].span.packed_record_start,
    );

    // Miss: nothing cached at all — sets pending, pushes one request.
    app.upgrade_active_override_to_complete();
    assert!(app.override_complete_pending);
    assert_eq!(app.override_inferred_raw.len(), 2, "unchanged on a miss");
    assert_eq!(app.heat_worker.as_ref().unwrap().queue_len(), 1);

    // Mismatch guard: `complete` holds a different range — the flag
    // must stay set and `override_inferred_raw` must stay untouched.
    app.heat_caches.lock().unwrap().complete = Some((
        range.start + 1000..range.start + 1001,
        vec![("pkg.Z".to_string(), 1)],
    ));
    app.poll_pending_override_work();
    assert!(
        app.override_complete_pending,
        "a mismatched complete range must not satisfy the pending request"
    );
    assert_eq!(app.override_inferred_raw.len(), 2);

    // `by_range` alone — even for the correct range — can never
    // satisfy the one-shot `[0, usize::MAX)` request.
    app.heat_caches.lock().unwrap().by_range.insert(
        range.start,
        RangeHeatEntry {
            best_score: Some(9),
            best_count: 1,
            top_n: vec![("pkg.C".to_string(), 9); 6],
        },
    );
    app.poll_pending_override_work();
    assert!(
        app.override_complete_pending,
        "by_range alone can never satisfy the one-shot full-list request"
    );
    assert_eq!(app.override_inferred_raw.len(), 2);

    // Only a `complete` hit for the exact range applies the full list
    // wholesale and clears the flag.
    app.heat_caches.lock().unwrap().complete =
        Some((range.clone(), vec![("pkg.C".to_string(), 9); 6]));
    app.poll_pending_override_work();
    assert!(!app.override_complete_pending);
    assert!(app.override_candidates_complete);
    assert_eq!(app.override_inferred_raw.len(), 6);
}

/// 2026-07-20 bug: `open_override_on_default`/`open_override_on_type`
/// (spec 0139) fall back from `Inferred` to `Lexicographic` mode
/// whenever the target key isn't found in whatever `Inferred`-mode
/// data was available *synchronously* — typically because the shared
/// cache is cold and the first `heat_lookup` merely queued a
/// background request rather than answering it. That fallback leaves
/// `override_candidates_pending` (or, via `upgrade_active_override_
/// to_complete`, `override_complete_pending`) set even though the
/// pane has since moved on to `Lexicographic`. Before the fix, once
/// that background request resolved, `poll_pending_override_work`
/// unconditionally overwrote `override_candidates` with the (capped)
/// `Inferred` raw list regardless of the now-active sort mode — so
/// the pane silently froze at whatever page height the truncated
/// `Inferred` list happened to have, with `override_sort` still
/// reporting `Lexicographic` and no growth logic ever engaging again
/// (`move_override_highlight`/`upgrade_active_override_to_complete`
/// only grow in `Inferred` mode).
#[test]
fn poll_pending_override_work_does_not_clobber_a_non_inferred_sort_mode() {
    let mut app = message_node_app_with_graph();
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.override_target = Some(0);
    app.override_list_height = 4;

    // Reproduces the fallback: an `Inferred`-mode miss leaves `pending`
    // set, then the pane falls back to `Lexicographic` without
    // clearing it — exactly what `open_override_on_default`/`open_
    // override_on_type` do on a cold cache.
    app.override_sort = SortMode::Inferred;
    app.recompute_override_candidates();
    assert!(app.override_candidates_pending);
    assert!(app.override_candidates.is_empty());

    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();
    let lexicographic_candidates = app.override_candidates.clone();
    assert!(!lexicographic_candidates.is_empty());
    assert!(
        app.override_candidates_pending,
        "the stale Inferred-mode pending flag must survive the fallback"
    );

    // The background request the first `recompute_override_candidates`
    // call queued now resolves — `by_range` covers the requested page.
    let idx = 0;
    let range = extract::message_payload_range(
        &app.blob,
        &app.tree[idx].span.raw_range,
        app.tree[idx].span.packed_record_start,
    );
    app.heat_caches.lock().unwrap().by_range.insert(
        range.start,
        RangeHeatEntry {
            best_score: Some(9),
            best_count: 1,
            top_n: vec![("pkg.Type".to_string(), 9); 4],
        },
    );

    app.poll_pending_override_work();
    assert!(!app.override_candidates_pending);
    assert_eq!(
        app.override_candidates, lexicographic_candidates,
        "resolving a stale Inferred fetch must not clobber the on-screen \
         Lexicographic list"
    );
    assert_eq!(
        app.override_inferred_raw.len(),
        4,
        "the resolved data must still be cached for a later 'i' toggle"
    );
}

/// 2026-07-20 feedback ("the pane is too lazy... it should get the
/// total number of candidates and update multiple times until it gets
/// them all"): opening the override pane must eagerly fetch the
/// *complete* candidate list on its own — a bounded poll of `poll_
/// pending_override_work` alone (no `move_override_highlight` calls
/// at all) is enough to reach `override_candidates_complete`, since
/// `upgrade_active_override_to_complete` now requests the whole,
/// unbounded list in one shot (`[0, usize::MAX)`) rather than growing
/// page by page. Real worker thread, real tiny in-memory graph
/// (`test_scoring_graph`, `HEAT_CUE_PREVIEW` == 8 message types) end-
/// to-end, mirroring `heat_cue_for_resolves_once_a_real_worker_
/// populates_the_cache`'s own pattern.
#[test]
fn override_pane_auto_completes_from_polling_alone_without_scrolling() {
    let mut app = message_node_app_with_graph();
    // `App::new` auto-seeds an active root override entry (`seed_root`)
    // whenever the fixture's root type resolves, which would otherwise
    // route `t` through `open_override_on_type`'s fallback instead of
    // the `open_override_on_default` path this test targets.
    app.overrides = OverrideCollection::new();
    // Likewise, the fixture's own node carries a resolved `type_fqdn`
    // (2026-07-20 fix: Step B.5 now seeds straight from a message
    // node's own `span.type_fqdn`, not just an override entry) — clear
    // it too, so the cursor node is genuinely typeless as well as
    // override-less, exercising `open_override_on_default` in
    // isolation.
    app.tree[0].span.type_fqdn = None;
    // Four repeated, structurally valid field-1 varint encodings — an
    // all-zero payload's leading tag byte (field number 0) is
    // structurally invalid and would veto every candidate.
    app.blob = vec![0x22, 0x08, 0x08, 0x01, 0x08, 0x02, 0x08, 0x03, 0x08, 0x04];
    app.splash = false;
    app.term_width = 120;
    // Smaller than the graph's 8 real candidates, so the fast bounded
    // first page (`recompute_override_candidates`) and the full-list
    // fetch (`upgrade_active_override_to_complete`) are genuinely
    // distinct requests.
    app.override_list_height = 2;

    let graph = app.ctx.graph.as_ref().unwrap().graph;
    let blob = Arc::new(app.blob.clone());
    let (tx, _rx) = mpsc::channel();
    app.heat_worker = Some(HeatWorkerHandle::spawn(
        Arc::clone(&app.heat_caches),
        graph,
        blob,
        tx,
    ));

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Inferred);
    assert!(!app.override_candidates_complete);

    // Bounded poll, not `recv` — this isn't exercising the
    // event-driven wiring, just the worker/cache-recheck contract.
    let mut resolved = false;
    for _ in 0..200 {
        app.poll_pending_override_work();
        if app.override_candidates_complete {
            resolved = true;
            break;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        resolved,
        "the real worker must resolve the full list within the bounded \
         poll, with no scrolling involved"
    );
    assert_eq!(app.override_candidates.len(), 8);
}

/// 2026-07-20 feedback, second half ("...PLUS try to put the cursor
/// on the correct one, when it has been fetched"): opening the pane
/// on a node with an existing active override whose type isn't in the
/// fast, bounded first page must still land the highlight on that
/// type once the full list arrives — driven purely by `poll_pending_
/// override_work`'s `override_seek_target` retry (`open_override_on_
/// type`), no scrolling.
#[test]
fn override_pane_seeks_the_active_overrides_type_once_the_complete_list_arrives() {
    let mut app = message_node_app_with_graph();
    app.blob = vec![0x22, 0x08, 0x08, 0x01, 0x08, 0x02, 0x08, 0x03, 0x08, 0x04];
    app.splash = false;
    app.term_width = 120;
    app.override_list_height = 2;

    let idx = 0;
    let origin_path = app.positional_path(idx);
    app.overrides.activate(
        OverrideOrigin::Path { path: origin_path },
        Some("Msg7".to_string()),
    );

    let graph = app.ctx.graph.as_ref().unwrap().graph;
    let blob = Arc::new(app.blob.clone());
    let (tx, _rx) = mpsc::channel();
    app.heat_worker = Some(HeatWorkerHandle::spawn(
        Arc::clone(&app.heat_caches),
        graph,
        blob,
        tx,
    ));

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Inferred);

    let mut resolved = false;
    for _ in 0..200 {
        app.poll_pending_override_work();
        if app.override_seek_target.is_none() {
            resolved = true;
            break;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        resolved,
        "the seek target must resolve within the bounded poll"
    );
    assert_eq!(
        app.override_sort,
        SortMode::Inferred,
        "Msg7 is a real inferred candidate — no Lexicographic fallback needed"
    );
    assert_eq!(
        app.override_candidates[app.override_highlight].0, "Msg7",
        "the highlight must land on the previously-active type once fetched"
    );
}

/// Spec 0161 G1/G2: successive live previews truncate the *previous*
/// preview's disposable subtree back to the session's watermark before
/// splicing the newly highlighted candidate in — so `self.tree`'s
/// growth across previews is bounded by the current splice's own size,
/// not by the sum of every candidate previewed so far this session.
#[test]
fn preview_override_highlight_truncates_prior_preview_before_splicing_next() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("test.Outer".to_string(), None),
    ];

    app.override_highlight = 0;
    app.preview_override_highlight();
    let watermark = app
        .preview_tree_watermark
        .expect("watermark must be captured on the first preview");
    let len_after_a = app.tree.len();
    assert!(len_after_a >= watermark);

    app.override_highlight = 1;
    app.preview_override_highlight();
    assert!(
        app.tree.len() >= watermark,
        "second preview must still start from the watermark, not from len_after_a"
    );

    // Same candidate/bytes as the first preview, spliced from the same
    // watermark again — must exactly reproduce A's own tree length, not
    // accumulate A + B + A.
    app.override_highlight = 0;
    app.preview_override_highlight();
    assert_eq!(app.tree.len(), len_after_a);
}

/// Spec 0161 G1 (second bullet of the Test plan): cycling through many
/// previews in a loop must not accumulate growth proportional to the
/// number of cycles — `self.tree.len()` must settle back to the same
/// bounded value every time the same candidate is re-previewed.
#[test]
fn repeated_previews_do_not_accumulate_tree_growth() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("test.Outer".to_string(), None),
    ];

    app.override_highlight = 0;
    app.preview_override_highlight();
    let len_after_single_preview = app.tree.len();

    for i in 0..40 {
        app.override_highlight = i % 2;
        app.preview_override_highlight();
    }
    app.override_highlight = 0;
    app.preview_override_highlight();

    assert_eq!(
        app.tree.len(),
        len_after_single_preview,
        "40 cycles of preview-switching must not accumulate tree growth"
    );
}

/// Spec 0161 G5: a node folded while it's part of the currently
/// previewed subtree must not survive as a stale index in `self.folded`
/// once a later preview truncates that subtree away.
#[test]
fn preview_truncation_drops_stale_folded_entries() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("test.Outer".to_string(), None),
    ];

    app.override_highlight = 0;
    app.preview_override_highlight();
    let watermark = app.preview_tree_watermark.unwrap();
    let child_to_fold = app.tree[inner_idx]
        .first_child
        .expect("previewing test.Inner over id: 5 must decode a child field");
    assert!(
        child_to_fold >= watermark,
        "the folded node must belong to the just-previewed (disposable) subtree"
    );
    app.folded.insert(child_to_fold);

    // A different preview triggers truncation back to `watermark`,
    // which must also drop `child_to_fold` from `folded` — no stale
    // index left behind.
    app.override_highlight = 1;
    app.preview_override_highlight();
    assert!(!app.folded.contains(&child_to_fold));
}

/// Spec 0161 hidden-flaw fix: if a preview's truncation runs but the
/// *following* `splice_override` call then fails (e.g. an unresolvable
/// candidate type), `idx`'s `first_child`/`last_child`/`doc_next` must
/// not keep dangling, out-of-bounds indices into the just-truncated
/// range — they must be left in a safe, merely-childless state.
#[test]
fn preview_override_highlight_survives_a_failing_splice_after_truncation() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("nonexistent.Type".to_string(), None),
    ];

    app.override_highlight = 0;
    app.preview_override_highlight();
    assert!(app.message.is_empty(), "first preview must succeed cleanly");

    // Second preview's candidate type doesn't exist in the descriptor
    // pool — `splice_override` returns `Err` after truncation already
    // ran.
    app.override_highlight = 1;
    app.preview_override_highlight();
    assert!(
        app.message.contains("cannot preview override"),
        "unresolvable candidate must report an error: {}",
        app.message
    );
    assert_eq!(app.tree[inner_idx].first_child, None);
    assert_eq!(app.tree[inner_idx].last_child, None);
    assert_eq!(app.tree[inner_idx].doc_next, None);

    // Must not panic walking `idx`'s (now empty) children.
    let mut child = app.tree[inner_idx].first_child;
    let mut count = 0;
    while let Some(c) = child {
        count += 1;
        child = app.tree[c].next_sibling;
    }
    assert_eq!(count, 0);
}

/// Spec 0161 regression: confirming an override with `Enter` after
/// several intervening live previews must produce identical final
/// content to confirming the same candidate directly, with no previews
/// in between — truncation must not interact with the real commit path
/// (`render_overrides`/`finalize_override_batch`), which is unaffected
/// by this spec.
#[test]
fn confirming_after_several_previews_matches_confirming_directly() {
    let (mut app_a, inner_idx_a, _) = type_as_fixture();
    app_a.cursor = inner_idx_a;
    app_a.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app_a.override_sort = SortMode::Lexicographic;
    app_a.recompute_override_candidates();
    let row_a = app_a
        .override_candidates
        .iter()
        .position(|(f, _)| f == "test.Inner")
        .expect("test.Inner must be a candidate");

    // Cycle through a few other rows first — each a real live preview
    // that truncates the previous one.
    for _ in 0..3 {
        app_a.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    app_a.override_highlight = row_a;
    app_a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let (mut app_b, inner_idx_b, _) = type_as_fixture();
    app_b.cursor = inner_idx_b;
    app_b.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app_b.override_sort = SortMode::Lexicographic;
    app_b.recompute_override_candidates();
    let row_b = app_b
        .override_candidates
        .iter()
        .position(|(f, _)| f == "test.Inner")
        .expect("test.Inner must be a candidate");
    app_b.override_highlight = row_b;
    app_b.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app_a.lines, app_b.lines);
    assert_eq!(
        app_a.tree[inner_idx_a].span.type_fqdn,
        app_b.tree[inner_idx_b].span.type_fqdn
    );
}

/// Spec 0161 regression: `Esc` after several live previews still
/// correctly reverts to the pre-preview content — unaffected by
/// truncation, since revert already goes through `render_overrides`,
/// not anything watermark-related.
#[test]
fn esc_after_several_previews_reverts_to_original_content() {
    let (mut app, inner_idx, _) = type_as_fixture();
    let original_lines = app.lines.clone();
    let original_type_fqdn = app.tree[inner_idx].span.type_fqdn.clone();

    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();

    for _ in 0..5 {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(app.lines, original_lines);
    assert_eq!(app.tree[inner_idx].span.type_fqdn, original_type_fqdn);
}
