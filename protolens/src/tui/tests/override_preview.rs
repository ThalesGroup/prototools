// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0185: the override pane's live preview — a render-time overlay
//! showing what a candidate would do, laid over a document it has to
//! leave exactly as it found it.

use super::super::*;
use super::support::*;

/// Spec 0185 G1/G2: a preview is a render-time overlay, so it touches
/// nothing the committed document is made of — and it is discarded by
/// assignment, with nothing to revert. Asserted rather than timed: a
/// timing test would pass for the wrong reasons on a fixture this
/// small.
#[test]
fn preview_override_highlight_touches_no_committed_state() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("test.Outer".to_string(), None),
    ];

    let tree_len = app.tree.len();
    let lines = app.document_lines().clone();
    let owners = line_owners(&app);
    let rows = app.visible_row_count();
    let folded = app.folded.clone();

    // Forty alternating previews: each one overwrites the overlay, and
    // nothing accumulates because nothing was ever spliced.
    for i in 0..40 {
        app.override_highlight = i % 2;
        app.preview_override_highlight();
        assert!(app.preview_overlay.is_some(), "candidate {i} must preview");
        assert_eq!(app.tree.len(), tree_len);
        assert_eq!(app.document_lines(), lines);
        assert_eq!(line_owners(&app), owners);
        assert_eq!(app.visible_row_count(), rows);
        assert_eq!(app.folded, folded);
    }

    app.close_override();
    assert!(app.preview_overlay.is_none());
    assert_eq!(app.tree.len(), tree_len);
    assert_eq!(app.document_lines(), lines);
    assert_eq!(line_owners(&app), owners);
    assert_eq!(app.visible_row_count(), rows);
    assert_eq!(app.folded, folded);
}

/// Spec 0185 G3, the acceptance criterion: the overlay's lines must be
/// exactly what a real `splice_override` of the same node under the
/// same candidate produces. This is the test that catches an S3
/// factoring which dropped the header patch, the truncation marker, or
/// the indentation level.
///
/// Spec 0187 S3 makes this the whole of the criterion: highlighting is
/// a pure function of the drawn rows' text, so equal lines *are* equal
/// colors. There is no longer a separate style vector that could agree
/// or disagree independently of the text.
#[test]
fn overlay_lines_match_the_committed_splice() {
    for candidate in ["test.Inner", "test.Outer"] {
        let (mut app_a, inner_idx_a, _) = type_as_fixture();
        app_a.override_target = Some(inner_idx_a);
        app_a.override_candidates = vec![(candidate.to_string(), None)];
        app_a.override_highlight = 0;
        app_a.preview_override_highlight();
        let overlay = app_a
            .preview_overlay
            .as_ref()
            .unwrap_or_else(|| panic!("{candidate} must preview: {}", app_a.message));

        let (mut app_b, inner_idx_b, _) = type_as_fixture();
        app_b
            .splice_override(inner_idx_b, Some(candidate.to_string()), false)
            .expect("the committed splice must succeed");
        let committed = app_b.node_lines(inner_idx_b);

        assert_eq!(
            overlay.lines,
            app_b.document_lines()[committed.clone()],
            "{candidate}: the overlay must render the committed splice's own lines"
        );
    }
}

/// Spec 0185 S2: the display-row map is arithmetic over one contiguous
/// substituted span. Checked at the boundaries — the row before
/// `first_row`, the first and last overlay rows, the first committed
/// row after, and one past the end — for an overlay shorter than,
/// equal to, and longer than the block it stands in for.
#[test]
fn display_row_map_holds_at_the_substitution_boundaries() {
    // Ten root-level one-line leaves, so visible row `r` is line `r` and
    // the arithmetic under test is the only thing the assertions can be
    // reading. The identity has to come from a real fixture: line
    // positions are derived from the tree's counters (spec 0210 S2), so
    // a test cannot simply assign itself one.
    let texts: Vec<String> = (0..10).map(|i| format!("l{i}: {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut app = sibling_leaves_app(&refs);

    for overlay_len in [1usize, 3, 6] {
        app.preview_overlay = Some(PreviewOverlay {
            first_row: 4,
            covered_rows: 3,
            lines: vec!["x".to_string(); overlay_len],
            spans: Vec::new(),
            bytes: Vec::new(),
        });
        assert_eq!(app.composed_row_count(), 10 - 3 + overlay_len);
        assert_eq!(app.display_row(3).and_then(|r| r.committed_line()), Some(3));
        assert!(matches!(app.display_row(4), Some(DisplayRow::Overlay(0))));
        assert!(
            matches!(app.display_row(4 + overlay_len - 1), Some(DisplayRow::Overlay(i)) if i == overlay_len - 1)
        );
        // The first committed row after the substituted block is line
        // `4 + 3`, wherever the overlay's own length put it in composed
        // space.
        assert_eq!(
            app.display_row(4 + overlay_len)
                .and_then(|r| r.committed_line()),
            Some(7)
        );
        assert!(app.display_row(app.composed_row_count()).is_none());
    }
}

/// Spec 0185 S1 × spec 0184: previewing a packed run stands in for the
/// whole run, because the record is the addressable unit and the whole
/// record is what a commit would splice. Getting this wrong shows as a
/// preview that replaces one line of a run and leaves the rest.
///
/// Spec 0216 makes the run a single node drawing one row per element,
/// so "any element stands in for the run" is now structural rather than
/// a rule the preview has to honor; what is still worth asserting is
/// that the overlay anchors at the run's first row and covers all of
/// them.
#[test]
fn previewing_a_packed_run_covers_all_of_its_rows() {
    let (mut app, run, _tail, _a, _b) = packed_run_with_tail_fixture();
    let run_lines = app.node_lines(run);

    app.override_target = Some(run);
    app.override_candidates = vec![("uint64".to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();

    let o = app
        .preview_overlay
        .as_ref()
        .unwrap_or_else(|| panic!("the run must preview: {}", app.message));
    let first_row = o.first_row;
    let covered = o.covered_rows;
    // Spec 0210 S2: rows are walked, not indexed out of a vector.
    assert_eq!(
        app.visible_row_pos(first_row).map(|(_, line)| line),
        Some(run_lines.start),
        "the overlay must anchor at the run's first row"
    );
    let rest = app.visible_row_count() - first_row;
    assert_eq!(
        covered,
        app.visible_window(first_row, rest)
            .into_iter()
            .take_while(|(l, _)| *l < run_lines.end)
            .count(),
        "...and stand in for every row of the run"
    );
    assert!(covered > 1, "the fixture's run spans several rows");
}

/// Spec 0185 S4: the overlay has no node, so `folded` is never
/// consulted for it and never mutated by it. A folded target renders in
/// full for the overlay's lifetime and goes back to folded when the
/// overlay is dropped — no saved state, because none was changed.
#[test]
fn previewing_a_folded_target_renders_it_in_full_and_leaves_folded_alone() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    // Spec 0210 S2: folding through `toggle_fold` rather than by hand, so
    // that the line counters the row walk reads are refreshed with it.
    app.toggle_fold(inner_idx);
    assert!(app.folded.contains(&inner_idx), "the fixture must fold");
    let folded_before = app.folded.clone();
    let rows_before: Vec<usize> = app
        .visible_window(0, app.visible_row_count())
        .into_iter()
        .map(|(l, _)| l)
        .collect();

    app.override_target = Some(inner_idx);
    app.override_candidates = vec![("test.Inner".to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();

    let o = app.preview_overlay.as_ref().expect("must preview");
    assert!(
        o.lines.len() > o.covered_rows,
        "the overlay must render the folded target in full: {:?} over {} rows",
        o.lines,
        o.covered_rows
    );
    assert_eq!(app.folded, folded_before);
    assert_eq!(
        app.visible_window(0, app.visible_row_count())
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
        rows_before
    );

    app.close_override();
    assert!(app.preview_overlay.is_none());
    assert_eq!(app.folded, folded_before);
    assert_eq!(
        app.visible_window(0, app.visible_row_count())
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
        rows_before
    );
}

/// Spec 0185 S6: a candidate that fails to render leaves the overlay at
/// `None` and the main pane showing committed content — reached without
/// a partial mutation to back out, which is the whole point of the
/// overlay. The committed subtree the previous, successful preview
/// stood in for is untouched throughout.
#[test]
fn a_failing_candidate_drops_the_overlay_and_leaves_the_document_intact() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.override_target = Some(inner_idx);
    app.override_candidates = vec![
        ("test.Inner".to_string(), None),
        ("nonexistent.Type".to_string(), None),
    ];
    let lines = app.document_lines().clone();
    let first_child = app.first_child(inner_idx);

    app.override_highlight = 0;
    app.preview_override_highlight();
    assert!(app.message.is_empty(), "first preview must succeed cleanly");
    assert!(app.preview_overlay.is_some());

    // The second candidate's type doesn't exist in the descriptor pool.
    app.override_highlight = 1;
    app.preview_override_highlight();
    assert!(
        app.message.contains("cannot preview override"),
        "unresolvable candidate must report an error: {}",
        app.message
    );
    assert!(app.preview_overlay.is_none());
    assert_eq!(app.document_lines(), lines);
    assert_eq!(app.first_child(inner_idx), first_child);
}

/// Spec 0185 S5/G5: while the override-selection pane is open, focus is
/// locked to it — not by `Tab`, not by a main-pane click. The lock is
/// load-bearing rather than cosmetic: it is what keeps the overlay's
/// row anchor valid. Panning (G4) is deliberately exempt.
#[test]
fn the_selection_pane_holds_focus_but_still_lets_the_main_pane_pan() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_target.is_some());
    assert!(app.override_focus);

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.message.clear();
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.override_focus, "Tab must not leave the selection pane");
    assert_eq!(
        app.message, OVERRIDE_FOCUS_LOCK_MESSAGE,
        "...but it must say why, rather than reading as a broken key"
    );

    let cursor = app.cursor;
    app.select_anchor = None;
    app.message.clear();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: app.main_area.x + 1,
        row: app.main_area.y + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.override_focus, "a main-pane click must not steal focus");
    assert_eq!(app.cursor, cursor, "...nor move the cursor");
    assert_eq!(app.select_anchor, None, "...nor start a selection");
    assert_eq!(
        app.message, OVERRIDE_FOCUS_LOCK_MESSAGE,
        "...but it must say why"
    );

    // G4: a one-row-high pane is what gives this three-line fixture
    // something to scroll.
    app.main_area.height = 1;
    app.scroll.index = 0;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert!(app.scroll.index > 0, "Alt-Down must pan the main pane");
    assert_eq!(app.cursor, cursor);

    app.scroll.index = 0;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: app.main_area.x,
        row: app.main_area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.scroll.index, WHEEL_PAN_STEP, "the wheel must still pan");
    assert_eq!(app.cursor, cursor);
}

/// Spec 0193 S3 reverses spec 0185 S7/Q1: the statusline no longer
/// spells out the focus lock, because the user learns of it the moment
/// they try to focus the main pane and the command/message row answers
/// with `OVERRIDE_FOCUS_LOCK_MESSAGE`. What the statusline still owes
/// the reader is that the content on display is not the real document,
/// so the suffix survives — shortened to ` (preview)`, and now tied to
/// an overlay actually existing rather than to the pane being open.
#[test]
fn the_main_statusline_announces_the_focus_lock() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;

    let mut terminal = Terminal::new(TestBackend::new(200, 24)).unwrap();
    let main_statusline = |app: &App, terminal: &Terminal<TestBackend>| -> String {
        let buffer = terminal.backend().buffer();
        let y = app.main_area.y + app.main_area.height;
        (app.main_area.x..app.main_area.x + app.main_area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    };

    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!main_statusline(&app, &terminal).contains("preview"));

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = main_statusline(&app, &terminal);
    assert!(
        row.contains("(preview)"),
        "a live preview must say so: {row:?}"
    );
    assert!(
        !row.contains("locked"),
        "the lock is the command/message row's business, not the statusline's: {row:?}"
    );

    app.override_candidates = vec![("nonexistent.Type".to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();
    assert!(app.preview_overlay.is_none());
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = main_statusline(&app, &terminal);
    assert!(
        !row.contains("preview"),
        "a failed preview leaves the real document on display, so it claims nothing: {row:?}"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!main_statusline(&app, &terminal).contains("preview"));
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

    assert_eq!(app_a.document_lines(), app_b.document_lines());
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
    let original_lines = app.document_lines().clone();
    let original_type_fqdn = app.tree[inner_idx].span.type_fqdn;

    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.override_sort = SortMode::Lexicographic;
    app.recompute_override_candidates();

    for _ in 0..5 {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.override_target, None);
    assert_eq!(app.document_lines(), original_lines);
    assert_eq!(app.tree[inner_idx].span.type_fqdn, original_type_fqdn);
}
