// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_worker::{HeatRequest, HeatWorkerHandle};
use super::super::render::{window_styles_for, ACTIVITY_GLYPH};
use super::super::*;
use super::support::*;
use prototext_core::serialize::encode_text::annotation_start;

/// Regression test: a legitimately-empty decode (e.g. reopening an
/// extracted `google.protobuf.Empty`, or any all-default submessage —
/// decoding zero bytes yields zero `TreeNode`s, not an error) must not
/// panic on the first `render()` call or on keypresses, now that
/// `main.rs` no longer refuses to open such a blob.
#[test]
fn empty_tree_renders_and_handles_keys_without_panicking() {
    let decoded = Decoded {
        lines: Vec::new(),
        tree: Vec::new(),
        root_type: "google.protobuf.Empty".to_string(),
        blob: Vec::new(),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
    };
    let mut app = App::new(
        decoded,
        "empty.pb",
        PathBuf::from("empty.pb"),
        2,
        DescriptorContext::empty_for_test(),
        ThemeKind::Dark,
        None,
    );

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // Dismiss the startup splash first (any key), then exercise a
    // handful of keys that are unguarded `self.tree[...]` indexing
    // sites for a non-empty tree.
    app.splash = false;
    // `z` goes last: spec 0194 S6 made it a chord prefix, so anything
    // after it would be eaten as the chord's second half.
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Char('0'),
        KeyCode::Char('$'),
        KeyCode::Char('%'),
        KeyCode::Char(' '),
        KeyCode::Char('x'),
        KeyCode::End,
        KeyCode::Char('z'),
        KeyCode::Char('c'),
    ] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.should_quit);
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.should_quit);
}

/// Spec 0192 G1: a frame costs the same wherever the cursor is. The
/// regression this guards is `positional_path` becoming O(ordinal)
/// again — which it was, via `sibling_position`'s `prev_sibling` walk,
/// once per drawn row. On a 20 000-sibling document that made the last
/// screenful roughly four orders of magnitude more expensive to draw
/// than the first (measured on `googleapis.desc`: 80 us of override
/// resolution at the top, 17 080 us at the end).
///
/// A timing assertion, because the property is about cost and nothing
/// structural distinguishes the two frames. The bound is deliberately
/// loose — a regression here is not a 3x one, it is a 100x one — so
/// scheduler noise cannot produce a false failure.
#[test]
fn a_frame_costs_the_same_at_the_end_of_a_wide_document_as_at_the_start() {
    const N: usize = 20_000;
    let mut app = wide_sibling_scalars_app(N);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

    // Draw once before timing anything: the first frame of a fresh
    // `App` pays one-off warm-up (the startup render's line map, the
    // syntax-highlighting window's first fill) that belongs to neither
    // position.
    terminal.draw(|frame| app.render(frame)).unwrap();

    let time_here = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
        // Best of several: the minimum is the run least disturbed by
        // the scheduler, and it is the ratio of the two minima that
        // carries the signal.
        (0..5)
            .map(|_| {
                let t = Instant::now();
                terminal.draw(|frame| app.render(frame)).unwrap();
                t.elapsed()
            })
            .min()
            .expect("at least one sample")
    };

    // `render` clamps `scroll_offset` to the cursor itself
    // (render.rs:507), so moving the cursor is all it takes.
    app.cursor = 0;
    let at_start = time_here(&mut app, &mut terminal);

    app.cursor = N - 1;
    let at_end = time_here(&mut app, &mut terminal);

    assert!(
        at_end.as_nanos() <= at_start.as_nanos().saturating_mul(10),
        "drawing the end of a {N}-sibling document took {at_end:?} against \
         {at_start:?} at the start — a per-row cost that scales with the \
         cursor's ordinal position is back"
    );
}

/// Spec 0192 S2: the active-override check runs once per addressable
/// record, not once per drawn row — a packed run's N element rows are N
/// nodes but one record (spec 0184 S2), so they share one positional
/// path and one answer.
#[test]
fn the_override_check_runs_once_per_record_not_once_per_row() {
    let (app, elems, ..) = packed_run_with_tail_fixture();
    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();

    let (flags, resolutions) = app.override_bold_flags(&window);
    assert_eq!(flags.len(), window.len());

    // Or the count assertion below would pass vacuously.
    assert!(elems.len() > 1, "fixture must contain a multi-element run");

    // The whole answer, recomputed with no collapsing at all.
    let naive: Vec<bool> = window
        .iter()
        .map(|&row| match row {
            DisplayRow::Committed(l) => app
                .node_at_own_line(l)
                .is_some_and(|idx| app.resolve_active_override(idx).is_some()),
            DisplayRow::Overlay(_) => false,
        })
        .collect();
    assert_eq!(flags, naive, "collapsing must not change the answer");

    let rows_with_a_node = window
        .iter()
        .filter(|&&row| match row {
            DisplayRow::Committed(l) => app.node_at_own_line(l).is_some(),
            DisplayRow::Overlay(_) => false,
        })
        .count();
    assert!(
        resolutions < rows_with_a_node,
        "{resolutions} resolutions for {rows_with_a_node} rows carrying a \
         node — the per-row work did not collapse at all"
    );
}

/// A passive status message auto-dismisses once `MESSAGE_TIMEOUT` has
/// elapsed since it was set — detected by `track_message_timeout`
/// (called from `render`) noticing an expired `message_deadline`.
#[test]
fn message_auto_dismisses_after_timeout() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.message = "pattern not found: xyz".to_string();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.message_deadline.is_some());
    assert_eq!(app.message, "pattern not found: xyz");

    // Not yet expired: still showing on a later render.
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.message, "pattern not found: xyz");

    // Force expiry (real time never actually elapses in a unit test).
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.message.is_empty());
    assert!(app.message_deadline.is_none());
}

/// Item 13 of 2026-07-17 feedback: the startup splash auto-dismisses
/// once `SPLASH_TIMEOUT` has elapsed, in addition to its existing
/// keypress/mouse dismissal — detected by `track_splash_timeout`
/// (called from `render`) noticing an expired `splash_deadline`.
#[test]
fn splash_auto_dismisses_after_timeout() {
    let mut app = empty_app();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    assert!(app.splash);
    terminal.draw(|frame| app.render(frame)).unwrap();
    // Not yet expired: still showing on a later render.
    assert!(app.splash);

    // Force expiry (real time never actually elapses in a unit test).
    app.splash_deadline = Instant::now() - Duration::from_millis(1);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!app.splash);
}

/// Spec 0197 test 9 (§S3, second channel). A TUI launch is exactly the
/// case where nobody was watching stderr, so the splash pane repeats
/// the eager-fallback warning.
#[test]
fn the_eager_fallback_warning_reaches_the_splash_pane() {
    let mut app = eager_fallback_app();
    assert!(app.splash);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("warning:") && text.contains("index.rkyv"),
        "the splash must name the fallback: {text:?}"
    );
}

/// Spec 0197 test 10 (§S3, third channel). The splash pane is gone
/// after `SPLASH_TIMEOUT`, which is the case the status line covers: a
/// user who is still reading the document at t+10 s can still find out
/// why startup was slow.
///
/// The bound on this is spec 0147 G5, asserted below: the *first
/// keypress* clears the status line unconditionally, and that rule is
/// not carved out for this warning. So the third channel buys
/// persistence in wall-clock time, not persistence across interaction.
#[test]
fn the_eager_fallback_warning_survives_the_splash_timing_out() {
    let mut app = eager_fallback_app();
    assert!(
        app.message.contains("index.rkyv"),
        "App::new must seed the status line: {:?}",
        app.message
    );

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    app.splash_deadline = Instant::now() - Duration::from_millis(1);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!app.splash, "the splash must have timed out");

    assert!(
        app.message.contains("index.rkyv"),
        "the warning must outlive the pane that also showed it: {:?}",
        app.message
    );

    // ...but only until the user does something. Spec 0147 G5.
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(app.message.is_empty());
}

/// A message never auto-dismisses while the bottom bar is actively
/// serving as a text-entry prompt (`command_buffer`) or a pending `q`
/// quit confirmation — both are actively awaiting a keypress, unlike
/// a plain notice.
#[test]
fn message_is_not_dismissed_while_a_prompt_or_quit_confirm_is_active() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.message = "some notice".to_string();
    app.command_buffer = Some(String::new());
    terminal.draw(|frame| app.render(frame)).unwrap();
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        app.message, "some notice",
        "prompt active: must not dismiss"
    );

    app.command_buffer = None;
    app.quit_confirm = true;
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        app.message, "some notice",
        "quit_confirm active: must not dismiss"
    );
}

/// Spec 0133 G3/G4: the main-pane `a` key toggles display of each
/// line's trailing `#@ ...` annotation, purely at render time — the
/// underlying `self.lines`/`self.line_styles` are untouched, so
/// toggling `a` twice restores byte-for-byte identical rendering.
/// Distinct from the override pane's own `i` (candidate sort,
/// exercised above) and the manage pane's own `a` (entry active
/// toggle) — this fixture has neither pane open, so only the
/// main-pane binding is reachable.
#[test]
fn a_toggles_the_main_pane_annotation_display() {
    let line = "  id: 5  #@ int32 = 1".to_string();
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
        },
        parent: None,
        first_child: None,
        last_child: None,
        next_sibling: None,
        prev_sibling: None,
        doc_next: None,
        doc_prev: None,
        sibling_ordinal: 1,
        lines_total: 1,
        lines_visible: 1,
        rendered_as: None,
    };
    let decoded = Decoded {
        lines: vec![line.clone()],
        tree: vec![node],
        root_type: "test.Msg".to_string(),
        blob: vec![0x08, 0x05],
        wrapper_offset: 0,
        root_candidates: Vec::new(),
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

    // Spec 0193 S1: every row carries the two-column fold field, blank
    // here since this node has nothing to fold.
    assert!(app.annotations);
    assert_eq!(
        app.row_content(DisplayRow::Committed(0)),
        format!("  {line}")
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(!app.annotations);
    assert_eq!(app.row_content(DisplayRow::Committed(0)), "    id: 5");
    // Spec 0187 S3: `row_spans` reads `window_styles`, which is a
    // per-frame product of the rows being drawn, so the window has to be
    // established before the row's spans can be asked for. Index 0 is
    // this one-row window's only position.
    let window = [DisplayRow::Committed(0)];
    app.refresh_window_styles(&window);
    let spanned: String = app
        .row_spans(window[0], 0)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(spanned, "    id: 5");

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(app.annotations);
    assert_eq!(
        app.row_content(DisplayRow::Committed(0)),
        format!("  {line}")
    );
}

/// Spec 0113 D33: the bold override hint applies to a node's own
/// header and (when it has children) footer line, but must not
/// cascade to descendant lines.
#[test]
fn the_active_override_hint_marks_header_and_footer_but_not_children() {
    // `render`'s hoisted override pass (spec 0192 S2) asks exactly this
    // of every drawn row.
    fn bolded(app: &App, line_idx: usize) -> bool {
        app.node_at_own_line(line_idx)
            .is_some_and(|idx| app.resolve_active_override(idx).is_some())
    }

    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("type-as test.Inner");
    assert_eq!(
        app.tree[inner_idx].span.type_fqdn.as_deref(),
        Some("test.Inner")
    );

    let header_line = app.absolute_start(inner_idx);
    let footer_line = app.node_lines(inner_idx).end - 1;
    assert!(bolded(&app, header_line));
    assert!(bolded(&app, footer_line));

    let id_line = app.absolute_start(id_idx);
    assert!(!bolded(&app, id_line));
}

/// 2026-07-18 feedback: when the cursor rests on a node's own closing
/// `}` line (spec 0142), the status line's `L<n>` must report the
/// footer line's own number, not the header's.
#[test]
fn status_line_reports_the_footer_line_number_for_a_footer_resting_cursor() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let footer_line = app.node_lines(inner_idx).end - 1;
    app.cursor = inner_idx;
    app.cursor_footer = true;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
    assert!(
        text.contains(&format!("L{}/", footer_line + 1)),
        "status line must report the footer's own 1-based line number: {text:?}"
    );
}

/// Spec 0147 G6: `MESSAGE_TIMEOUT` is exactly 3 seconds (down from 4),
/// per the original proposal's stated value.
#[test]
fn message_timeout_is_three_seconds() {
    assert_eq!(MESSAGE_TIMEOUT, Duration::from_secs(3));
}

/// Spec 0147 G2: the main pane's local statusline shows a
/// right-flushed `[start..end)  L<curr>/<total>` ruler when it is the
/// only pane open (full width), but drops it entirely — not
/// truncates it — once a side pane is open and the main pane is only
/// half-width.
#[test]
fn main_statusline_omits_the_ruler_when_a_side_pane_is_open() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains(".."),
        "full width: the byte-range ruler must be shown: {row_text:?}"
    );

    app.toggle_override();
    assert!(app.override_target.is_some());
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..app.main_area.width)
        .map(|x| {
            buffer[(app.main_area.x + x, statusline_row)]
                .symbol()
                .to_string()
        })
        .collect();
    assert!(
        !row_text.contains(".."),
        "half width: the byte-range ruler must be omitted: {row_text:?}"
    );
}

/// 2026-07-19 feedback item 7: the main pane's local statusline shows
/// the full path as given on the command line (`App::new`'s
/// `blob_path` argument), not just the short filename.
#[test]
fn main_statusline_shows_the_full_command_line_path_not_just_the_filename() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.blob_path = PathBuf::from("some/nested/dir/test.pb");

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains("some/nested/dir/test.pb"),
        "the statusline must show the full command-line path: {row_text:?}"
    );
}

/// Spec 0193 S3 revises spec 0147 G2's truncation rule: when the
/// terminal is too narrow for the local statusline's full left-hand
/// content, the *head* is dropped and a leading `<` marker announces
/// it, so the reader keeps the informative tail (the node's own name
/// and span, rather than the enclosing file path). The right-flushed
/// ruler remains shown in full either way.
#[test]
fn main_statusline_truncates_the_left_side_with_a_marker_when_narrow() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;

    let backend = TestBackend::new(30, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.starts_with('<'),
        "narrow terminal: the dropped head must be marked with a leading `<`: {row_text:?}"
    );
    assert!(
        row_text.contains("[message][2..4)"),
        "the left side's tail must survive the truncation: {row_text:?}"
    );
    assert!(
        !row_text.contains("test.pb"),
        "it is the head that is dropped, not the tail: {row_text:?}"
    );
    assert!(
        row_text.trim_end().ends_with("All"),
        "the right-flushed ruler must still be shown in full: {row_text:?}"
    );
}

/// Spec 0147 G3: the `Length(1)` vertical separator column between
/// the main pane and an open side pane is filled with `'│'` for the
/// full height of `main_outer`/`right_outer` (content rows plus each
/// pane's own local statusline row).
#[test]
fn vertical_separator_renders_between_main_and_side_pane() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.toggle_override();
    assert!(app.override_target.is_some());

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();

    let separator_x = app.side_area.x - 1;
    for y in app.main_area.y..=(app.main_area.y + app.main_area.height) {
        assert_eq!(
            buffer[(separator_x, y)].symbol(),
            "│",
            "separator column must render '│' at row {y}"
        );
    }
}

/// 2026-07-19 feedback item 5: local statuslines render in vim-style
/// inverted video (`Modifier::REVERSED`), and the focused pane's own
/// statusline uses a brighter accent (`Color::White`) than an
/// unfocused pane's (`Color::Gray`).
#[test]
fn local_statuslines_are_reversed_and_the_focused_pane_is_brighter() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.toggle_override();
    assert!(app.override_target.is_some());
    assert!(app.override_focus);

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();

    let main_statusline_row = app.main_area.y + app.main_area.height;
    let main_cell = &buffer[(app.main_area.x, main_statusline_row)];
    assert!(
        main_cell.modifier.contains(Modifier::REVERSED),
        "unfocused main pane's statusline must be reversed"
    );
    assert_eq!(
        main_cell.fg,
        Color::Gray,
        "unfocused pane's statusline accent must be the dimmer gray"
    );

    let side_statusline_row = app.side_area.y + app.side_area.height;
    let side_cell = &buffer[(app.side_area.x, side_statusline_row)];
    assert!(
        side_cell.modifier.contains(Modifier::REVERSED),
        "focused override pane's statusline must be reversed"
    );
    assert_eq!(
        side_cell.fg,
        Color::White,
        "focused pane's statusline accent must be the brighter white"
    );
}

/// Spec 0147 G4: the global command/message row is always reserved at
/// a fixed `Length(1)` height, regardless of whether it is blank,
/// showing a passive message, or showing active command entry — the
/// main content area must never resize because of it.
#[test]
fn global_command_message_row_stays_fixed_height() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.cmd_area.is_none());
    let main_height = app.main_area.height;

    app.message = "some notice".to_string();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.cmd_area.unwrap().height, 1);
    assert_eq!(
        app.main_area.height, main_height,
        "main content area must not resize when a message appears"
    );

    app.message.clear();
    app.command_buffer = Some("cmd".to_string());
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.cmd_area.unwrap().height, 1);
    assert_eq!(
        app.main_area.height, main_height,
        "main content area must not resize during active command entry"
    );
}

// ── Spec 0190: the activity dot ──────────────────────────────────────────

/// Spec 0190 test plan item 5 / spec 0191 test plan item 5. The dot
/// occupies column 0 of the bottom row and renders `App::activity_shown`
/// — the value `run_loop` decided — rather than probing the worker's
/// atomics at draw time (0191 S2). Installing a worker and pushing to it
/// must therefore *not* change the cell on its own; only the field does.
///
/// The color is asserted, not just the glyph: `heat_style`'s levels are
/// all >= 4 precisely so this cell is never blank-because-invisible on
/// an ANSI-16 terminal, and the tiers must stay distinguishable there.
#[test]
fn the_activity_dot_renders_the_decided_level_in_column_zero() {
    let mut app = message_node_app();
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    let dot = (0u16, 9u16); // column 0 of the bottom (global) row

    // Nothing decided yet — the cell is blank and the command row still
    // starts one column in.
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "no decided activity means nothing to report"
    );

    // A live worker with a queued request is *not* enough: the dot is no
    // longer a probe. This is the assertion that pins 0191 S2 — it fails
    // against the pre-0191 implementation, which read `heat_activity()`
    // here and would light the cell from the render's own cue lookups.
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_worker.as_ref().unwrap().push(
        HeatRequest {
            range: usize::MAX - 1..usize::MAX,
            current_key: None,
            start: 0,
            end: 1,
            tier: tiered::Tier::User,
        },
        tiered::Tier::User,
    );
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "a queued request the loop has not yet accounted for must not light the dot"
    );

    app.activity_shown = Some(tiered::Tier::Visible);
    terminal.draw(|frame| app.render(frame)).unwrap();
    let visible_style = terminal.backend().buffer()[dot].style();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);

    app.activity_shown = Some(tiered::Tier::User);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);
    assert_ne!(
        terminal.backend().buffer()[dot].style(),
        visible_style,
        "User must outrank Visible and look different, ANSI-16 included"
    );
}

/// Test plan item 6. Reserving column 0 must move the command row, and
/// everything derived from it, one column right — the cursor included.
/// This is the assertion that would catch an implementation that inset
/// the dot at each use site instead of re-binding `cmd_row`.
#[test]
fn reserving_the_dot_column_shifts_the_command_row_and_its_cursor() {
    let mut app = message_node_app();
    app.splash = false;
    app.command_buffer = Some("cmd".to_string());
    app.command_cursor = 3;
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();

    terminal.draw(|frame| app.render(frame)).unwrap();
    let cmd_area = app.cmd_area.expect("an active command buffer draws a row");
    assert_eq!(cmd_area.x, 1, "the command row starts after the dot column");
    assert_eq!(cmd_area.width, 79, "and is one column narrower");
    assert_eq!(
        terminal.backend().buffer()[(1u16, 9u16)].symbol(),
        ":",
        "the command prefix must land in column 1, not column 0"
    );
    // ":cmd" with the cursor past the last char => column 1 + 4.
    assert_eq!(terminal.get_cursor_position().unwrap().x, 5);
}

// ── Spec 0187: highlighting is a property of the viewport ────────────────

/// Test plan item 1. The whole point of S3's synthetic enclosing
/// context: a window parsed on its own must be colored exactly as those
/// same lines are colored inside the complete document. Checked for
/// *every* scroll offset and every window height, because the failure
/// mode is offset-dependent — a window that happens to start at depth
/// zero passes even with no framing at all.
#[test]
fn window_highlighting_matches_whole_document_highlighting() {
    let app = nested_message_set_fixture();
    let lines = &app.lines;
    let document = colorize::hints_by_line(lines, &colorize::colorize(&lines.join("\n")));

    for height in 1..=lines.len() {
        for start in 0..=(lines.len() - height) {
            let window = lines[start..start + height].to_vec();
            let got = window_styles_for(&window, app.indent_size);
            assert_eq!(
                got,
                document[start..start + height],
                "window {start}..{} (height {height}) must be colored as the \
                 document colors those same lines; window text was {window:#?}",
                start + height
            );
        }
    }
}

/// Test plan item 2. The synthetic openers and closers S3 wraps the
/// window in are parser scaffolding, not content: they must not appear
/// as extra buckets, and no hint may be attributed to a line it does not
/// fit inside. Exercised from a window that starts at the deepest line
/// of the fixture, where the scaffolding is largest.
#[test]
fn the_synthetic_context_is_dropped_not_drawn() {
    let app = nested_message_set_fixture();
    let deepest = app
        .lines
        .iter()
        .enumerate()
        .max_by_key(|(_, l)| l.len() - l.trim_start().len())
        .map(|(i, _)| i)
        .expect("the fixture has lines");

    for height in 1..=(app.lines.len() - deepest) {
        let window = app.lines[deepest..deepest + height].to_vec();
        let styles = window_styles_for(&window, app.indent_size);
        assert_eq!(
            styles.len(),
            window.len(),
            "one bucket per drawn row, with the {height}-row window at line {deepest}"
        );
        for (line, hints) in window.iter().zip(&styles) {
            for (range, role) in hints {
                assert!(
                    range.end <= line.len(),
                    "hint {range:?} ({role:?}) runs past the end of {line:?}"
                );
            }
        }
    }
}

/// Test plan item 1b. `insert_truncation_marker` writes a literal `...`
/// row, which is not in the prototext grammar. Before this spec that was
/// harmless because the marker was spliced in *after* the document was
/// colorized; now the marker can be inside the window being parsed.
///
/// Left unblanked it does not merely fail to color itself —
/// tree-sitter's error recovery swallows *following* siblings (see
/// `colorize::bare_decimal_field_name_does_not_corrupt_sibling_captures`),
/// so the rows *beneath* it would silently lose their colors. That is
/// the assertion here: the rows after the marker are colored exactly as
/// they are with no marker present at all.
#[test]
fn a_truncation_marker_does_not_decolor_the_rows_beneath_it() {
    let mut app = nested_message_set_fixture();
    // Cut the fixture just after a value line, so the marker lands in
    // the middle of the window with several colorable rows below it.
    let cut = 8;
    let without = app.lines.clone();
    let expected = window_styles_for(&without, app.indent_size);

    let mut with: Vec<String> = without[..cut].to_vec();
    with.push("            ...".to_string());
    with.extend(without[cut..].iter().cloned());

    let rows: Vec<DisplayRow> = (0..with.len()).map(DisplayRow::Overlay).collect();
    app.preview_overlay = Some(PreviewOverlay {
        first_row: 0,
        covered_rows: 0,
        lines: with.clone(),
    });
    app.refresh_window_styles(&rows);
    let got = &app.window_styles;

    assert_eq!(got.len(), with.len(), "one bucket per drawn row");
    assert!(
        got[cut].is_empty(),
        "the marker row itself is not prototext and gets no hints, got {:?}",
        got[cut]
    );
    assert_eq!(
        &got[..cut],
        &expected[..cut],
        "the rows above the marker are unaffected"
    );
    assert_eq!(
        &got[cut + 1..],
        &expected[cut..],
        "the rows below the marker must be colored exactly as they are \
         when no marker is present — this is the assertion tree-sitter's \
         error recovery breaks if the marker reaches the parser"
    );
}

/// Test plan item 3. Hiding annotations is now driven by the format's
/// own `  #@ ` rule (`prototext_core::serialize::encode_text::
/// annotation_start`) rather than by the highlighter's Comment capture.
/// The case that separates a correct implementation from a naive one is
/// a line whose *string value* itself contains a literal `#@`: only a
/// rightmost-first scan with a leftward fallback gets it right.
#[test]
fn hiding_annotations_respects_a_hash_at_inside_a_string_value() {
    let line = "  name: \"a  #@ b\"  #@ string = 2".to_string();
    let mut app = sibling_leaves_app(&["x"]);
    app.lines = vec![line.clone()];
    app.annotations = false;
    // Spec 0193 S1's blank fold field accounts for the two extra columns.
    assert_eq!(
        app.row_content(DisplayRow::Committed(0)),
        "    name: \"a  #@ b\""
    );
    app.annotations = true;
    assert_eq!(
        app.row_content(DisplayRow::Committed(0)),
        format!("  {line}")
    );
}

/// Test plan item 3b. The one place this spec knowingly changes what the
/// user sees. An empty packed record renders as a comment-only line
/// (`render_text/packed.rs`'s `pack_size == 0` arm: indentation, then
/// the annotation with no value token before it). The old rule cut at
/// the highlighter's Comment capture and then trimmed, leaving `""`
/// only by accident of the trim; the format's rule reaches the same
/// answer deliberately, because the two separator spaces it excludes
/// *are* the line's whole indentation at level 1.
///
/// A test rather than a comment because someone reading a blank row
/// might reasonably "fix" it back into a run of spaces.
#[test]
fn an_empty_packed_record_row_hides_to_nothing() {
    let mut app = sibling_leaves_app(&["x"]);
    app.lines = vec!["  #@ = 7 pack_size: 0".to_string()];
    app.annotations = false;
    assert_eq!(app.row_content(DisplayRow::Committed(0)), "");
}

/// Test plan item 3c. The rule moved across a crate boundary, from the
/// highlighter's Comment capture to the encoder's own byte scan. Pin
/// that the move is behavior-preserving: over real renderer output, the
/// old rule (first Comment hint's start, then `trim_end`) and the new
/// one must pick the same truncation point on every line.
#[test]
fn the_format_rule_and_the_parser_rule_agree_on_rendered_lines() {
    for app in [nested_message_set_fixture(), nested_any_fixture()] {
        let per_line =
            colorize::hints_by_line(&app.lines, &colorize::colorize(&app.lines.join("\n")));
        for (line, hints) in app.lines.iter().zip(&per_line) {
            let old = hints
                .iter()
                .find(|(_, role)| *role == SyntaxRole::Comment)
                .map(|(range, _)| line[..range.start].trim_end())
                .unwrap_or(line.as_str());
            let new = match annotation_start(line) {
                Some(pos) => &line[..pos],
                None => line.as_str(),
            };
            assert_eq!(new, old, "the two rules disagree on {line:?}");
        }
    }
}

/// Test plan item 4. `row_content` no longer consults any hint vector,
/// which is what lets it keep working on lines that are nowhere near
/// the viewport. Selecting a range entirely below the visible window and
/// copying it must still strip annotations — under the old rule this
/// read `line_styles` at an index the viewport had never populated.
#[test]
fn clipboard_copy_strips_annotations_outside_the_viewport() {
    let mut app = nested_message_set_fixture();
    app.annotations = false;
    // A window covering only the first two rows; everything selected
    // below is outside it.
    app.refresh_window_styles(&[DisplayRow::Committed(0), DisplayRow::Committed(1)]);

    let last = app.lines.len() - 1;
    app.select_anchor = Some(last - 3);
    app.select_end = Some(last);
    let (count, copied) = app.selected_text().expect("the selection must yield text");

    assert_eq!(count, 4);
    for line in copied.lines() {
        assert!(
            !line.contains("#@"),
            "annotations must be stripped outside the viewport too, got {line:?}"
        );
    }
}

/// Draw one frame of `app`, splash dismissed. Spec 0194 draws the caret
/// in `render`, over the finished span list, so its assertions have to
/// read the frame rather than `row_spans`.
fn drawn_frame(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    terminal
}

/// Every main-pane cell of a drawn frame whose style satisfies `pick`,
/// as `(committed line index, symbol)`.
fn marked_cells(
    app: &App,
    terminal: &Terminal<TestBackend>,
    pick: impl Fn(Style) -> bool,
) -> Vec<(usize, String)> {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    let mut found = Vec::new();
    for y in area.y..area.y + area.height {
        let Some((_, line)) = app.visible_row_pos(app.scroll_offset + (y - area.y) as usize) else {
            continue;
        };
        for x in area.x..area.x + area.width {
            let cell = &buffer[(x, y)];
            if pick(cell.style()) {
                found.push((line, cell.symbol().to_string()));
            }
        }
    }
    found
}

/// The cells drawn in `theme::caret_style` — the one inverted character
/// (spec 0194 S2).
fn caret_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    marked_cells(app, terminal, |s| {
        s.add_modifier.contains(Modifier::REVERSED)
    })
}

/// The cells drawn in `theme::caret_paired_style` — the caret dimmed to
/// a tint because its matching brace is on screen carrying the strong
/// cue instead (spec 0194 S4).
fn paired_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    let want = crate::theme::caret_paired_style(app.theme).bg;
    marked_cells(app, terminal, move |s| s.bg == want)
}

/// Put the caret on one member of the cursor node's brace pair.
fn put_caret_on_brace(app: &mut App, closing: bool) -> (usize, usize) {
    let (open, close) = app
        .cursor_brace_pair()
        .expect("the cursor node must be bracketed");
    let (line, column) = if closing { close } else { open };
    app.cursor_footer = line != app.absolute_start(app.cursor);
    app.cursor_column = column;
    (line, column)
}

/// The display column `needle` is drawn in, in characters — not bytes,
/// since the fold marker is three bytes wide and one column wide.
fn column_of(content: &str, needle: &str) -> usize {
    let byte = content
        .find(needle)
        .unwrap_or_else(|| panic!("{content:?} must contain {needle:?}"));
    content[..byte].chars().count()
}

/// Spec 0193 test-plan items 1 and 2 (G1, G2). `nested_any_fixture`
/// renders `type_url: "..."` and `value {` as siblings at the same
/// depth — one with nothing to fold, one foldable — which is exactly
/// the pair the complaint was about: today the second one's name sits
/// one column right of the first's.
#[test]
fn the_fold_marker_does_not_displace_the_row_it_marks() {
    let app = nested_any_fixture();
    let line_of = |needle: &str| {
        app.lines
            .iter()
            .position(|l| l.trim_start().starts_with(needle))
            .unwrap_or_else(|| panic!("fixture must render a {needle:?} line"))
    };
    let plain = app.row_content(DisplayRow::Committed(line_of("type_url:")));
    let foldable = app.row_content(DisplayRow::Committed(line_of("value {")));
    assert_eq!(
        column_of(&plain, "type_url"),
        column_of(&foldable, "value"),
        "a foldable row's text must start in the same column as its \
         non-foldable sibling's: {plain:?} against {foldable:?}"
    );

    // Item 2, at both ends of the S1 rule: `value {` is indented deeply
    // enough for the marker to take the last two columns of its own
    // indentation, while the root row has no indentation at all and
    // falls back to the reserved field.
    let root = app.row_content(DisplayRow::Committed(0));
    for (content, token) in [(&foldable, "value"), (&root, "1 {")] {
        assert_eq!(
            column_of(content, "\u{25be}") + render::FOLD_FIELD_WIDTH,
            column_of(content, token),
            "the marker must sit immediately left of the token: {content:?}"
        );
    }
}

/// Spec 0193 test-plan item 3 (G6). `marker_column` is what the mouse's
/// fold hit test steers by, so it must report the column the glyph is
/// actually drawn in — for every `--indent`, not just the default 2.
/// The `1` case is the one that discriminates: the marker cannot fit in
/// a one-column indent, so it falls back into the reserved field and
/// its column stops tracking the indent.
#[test]
fn marker_column_agrees_with_what_is_drawn_at_every_indent_width() {
    for width in [0usize, 1, 2, 4] {
        let mut app = nested_any_fixture();
        // Re-indent the document as `--indent width` would have. Only
        // the leading whitespace changes, so every node's line range —
        // and therefore every fold marker — stays where it was.
        app.lines = app
            .lines
            .iter()
            .map(|l| {
                let depth = (l.len() - l.trim_start().len()) / 2;
                format!("{}{}", " ".repeat(depth * width), l.trim_start())
            })
            .collect();

        let window: Vec<DisplayRow> = (0..app.composed_row_count())
            .filter_map(|d| app.display_row(d))
            .collect();
        app.refresh_window_styles(&window);
        for (i, &row) in window.iter().enumerate() {
            let DisplayRow::Committed(line) = row else {
                continue;
            };
            let drawn: String = app
                .row_spans(row, i)
                .iter()
                .map(|s| s.content.to_string())
                .collect();
            let Some(marker) = drawn.chars().position(|c| c == '\u{25be}') else {
                continue;
            };
            assert_eq!(
                marker,
                render::marker_column(&app.lines[line]) as usize,
                "--indent {width}, line {line}: the hit test and the drawn \
                 glyph must agree on the column: {drawn:?}"
            );
        }
    }
}

/// Spec 0193 test-plan item 4. Spec 0185 G3 requires a preview and the
/// commit that follows it to be byte-identical, which rests on
/// `row_content` and `row_spans` staying in step; S1 and S2 edit both.
/// The fixture is folded partway through so the collapse summary, the
/// margin and the brace highlight all appear.
#[test]
fn row_content_and_row_spans_agree_byte_for_byte() {
    let (mut app, items) = repeated_message_fixture();
    app.toggle_fold(items[1]);
    app.cursor = items[0];

    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();
    app.refresh_window_styles(&window);
    assert!(window.len() > 4, "the fixture must have rows to compare");
    for (i, &row) in window.iter().enumerate() {
        let drawn: String = app
            .row_spans(row, i)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(drawn, app.row_content(row), "row {i} disagrees");
    }
}

/// Spec 0194 test-plan item 11 — spec 0193's brace-highlight test
/// rewritten rather than deleted. Spec 0193 lit both of the cursor
/// node's braces in a color of their own, all the time; spec 0194 keeps
/// only the pairing *the caret is standing on*, and splits the cue in
/// two so the strong half marks the brace the user is looking for.
/// The three states of S4's table, in order.
#[test]
fn a_brace_pairs_with_its_match_only_when_the_caret_is_on_it() {
    // Off a brace: one inverted cell, and no partner tint anywhere.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let header = app.absolute_start(items[1]);
    let first_non_blank = app.lines[header]
        .trim_start()
        .chars()
        .next()
        .expect("the header row is not blank")
        .to_string();
    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, first_non_blank)],
        "the caret alone, on the row's first non-blank"
    );
    assert!(
        paired_cells(&app, &terminal).is_empty(),
        "nothing is paired with a caret that is not on a brace"
    );

    // On a brace whose match is scrolled out of the window: the caret
    // keeps the strong cue, since there is nothing on screen to point
    // at with it.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[2]);
    let (footer, _) = put_caret_on_brace(&mut app, true);
    let mut terminal = drawn_frame(&mut app, 40, 8);
    // No cursor movement in between, so `render`'s auto-pan-into-view
    // guard leaves this alone.
    app.scroll_offset = footer;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(footer, "}".to_string())],
        "an unmatchable brace is drawn like any other caret"
    );
    assert!(paired_cells(&app, &terminal).is_empty());

    // On a brace whose match is in the window: the caret dims to a
    // tint and the *match* takes the inversion.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let (open_line, _) = put_caret_on_brace(&mut app, false);
    let close_line = app.node_lines(items[1]).end - 1;
    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "the strong cue belongs to the brace the user is looking for"
    );
    assert_eq!(
        paired_cells(&app, &terminal),
        vec![(open_line, "{".to_string())],
        "and the caret itself keeps only the tint"
    );
}

/// Spec 0194 test-plan item 12. Whether the match is on screen is a
/// property of the frame, not of the last keypress, so it has to be
/// re-resolved on every draw: panning the match off the left edge must
/// hand the strong cue back to the caret, and panning back must undo
/// that — with nothing that moves the cursor pressed in between.
#[test]
fn losing_sight_of_the_match_returns_the_strong_cue_to_the_caret() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let (open_line, _) = put_caret_on_brace(&mut app, false);
    let close_line = app.node_lines(items[1]).end - 1;
    let mut terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        paired_cells(&app, &terminal).len(),
        1,
        "paired to begin with"
    );

    // Pan just far enough to take the closing brace off the left edge.
    // The caret's own `{` is further right on a more deeply indented
    // row, so it survives the same pan and stays drawn.
    let (_, (_, close_column)) = app.cursor_brace_pair().expect("the node is bracketed");
    app.pan_offset = render::FOLD_FIELD_WIDTH + close_column + 1;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(open_line, "{".to_string())],
        "with the match panned out the caret takes the strong cue back"
    );
    assert!(paired_cells(&app, &terminal).is_empty());

    app.pan_offset = 0;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "and panning back restores the pair"
    );
    assert_eq!(
        paired_cells(&app, &terminal),
        vec![(open_line, "{".to_string())]
    );
}

/// Spec 0194 test-plan item 13. A folded node carries its whole pair on
/// one row, and the closing half of it is synthetic — text that exists
/// only as an insertion, at a byte offset `row_text` and `row_spans`
/// disagree about. Drawing the caret by character index over the
/// finished span list is what keeps the two halves apart here.
#[test]
fn a_folded_node_pairs_its_synthetic_closing_brace_on_the_same_row() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[0]);
    app.toggle_fold(items[0]);
    let header = app.absolute_start(items[0]);
    put_caret_on_brace(&mut app, false);

    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, "}".to_string())],
        "the collapse summary's closing brace is the match"
    );
    assert_eq!(
        paired_cells(&app, &terminal),
        vec![(header, "{".to_string())],
        "both halves are on the caret's row, styled differently"
    );
}

/// A drawable app whose rows read exactly `texts`.
///
/// `sibling_leaves_app` cannot be drawn: its `raw_range`s point into an
/// empty blob, and the heat cue's payload walk trips over that on the
/// first frame. This borrows `wide_sibling_scalars_app`'s real
/// two-byte-per-node blob — one node per row either way, so substituting
/// the text leaves every `text_range` valid.
fn text_rows_app(texts: &[&str]) -> App {
    let mut app = wide_sibling_scalars_app(texts.len());
    app.lines = texts.iter().map(|s| s.to_string()).collect();
    app
}

/// The character (not byte) column `needle` was drawn in. The heat and
/// fold glyphs are multi-byte, so a byte offset would be off by their
/// width.
fn drawn_column_of(row: &str, needle: &str) -> usize {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{row:?} must contain {needle:?}"));
    row[..byte].chars().count()
}

/// One drawn main-pane row, as a string — used where an assertion is
/// about *where* something landed on screen rather than how it is
/// styled.
fn drawn_row(app: &App, terminal: &Terminal<TestBackend>, row: u16) -> String {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    (area.x..area.x + area.width)
        .map(|x| buffer[(x, area.y + row)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The foregrounds of one drawn main-pane row.
fn drawn_row_fgs(app: &App, terminal: &Terminal<TestBackend>, row: u16) -> Vec<Color> {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    (area.x..area.x + area.width)
        .map(|x| buffer[(x, area.y + row)].fg)
        .collect()
}

/// Spec 0194 test-plan item 1 (G1). vim's caret, not vim's
/// `cursorline`: one cell inverted, and the rest of the row still in
/// the syntax colors it would have had with the cursor elsewhere.
#[test]
fn the_caret_covers_one_cell_and_leaves_the_rows_colors_alone() {
    let mut app = text_rows_app(&["a: 1", "b: 2"]);
    app.cursor = 0;
    app.reset_caret_column();
    let terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(
        caret_cells(&app, &terminal).len(),
        1,
        "one cell, not the whole row"
    );
    let with_caret = drawn_row_fgs(&app, &terminal, 0);

    app.cursor = 1;
    app.reset_caret_column();
    let terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(
        with_caret,
        drawn_row_fgs(&app, &terminal, 0),
        "the caret's row keeps every foreground it has without one"
    );
}

/// Spec 0194 test-plan item 2 (S2, G7). The caret's row gets vim's
/// `cursorline` tint, a drag selection gets the reversal it has always
/// had, and a row that is both stays readable: the caret cell's own
/// reversal cancels against the selection's, so the caret is still
/// visible rather than merging into the block.
#[test]
fn a_selected_row_and_the_caret_row_stay_distinguishable() {
    let mut app = text_rows_app(&["a: 1", "b: 2", "c: 3"]);
    app.cursor = 0;
    app.reset_caret_column();
    app.select_anchor = Some(0);
    app.select_end = Some(1);
    let terminal = drawn_frame(&mut app, 40, 6);

    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    // The heat-cue column, the fold field and the row's own four
    // characters — the cells the row's spans actually cover.
    let drawn = 1 + render::FOLD_FIELD_WIDTH as u16 + 4;
    let reversed = |row: u16, x: u16| {
        buffer[(area.x + x, area.y + row)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    };

    assert!(
        (0..drawn).all(|x| reversed(1, x)),
        "a selected row that is not the caret's is reversed throughout"
    );
    let plain: Vec<u16> = (0..drawn).filter(|&x| !reversed(0, x)).collect();
    assert_eq!(
        plain.len(),
        1,
        "on a row that is both, exactly the caret cell cancels back"
    );
    assert_eq!(
        buffer[(area.x, area.y)].style().bg,
        crate::theme::cursor_row_style(app.theme).bg,
        "and the caret's row carries the cursorline tint underneath"
    );
    assert!(
        !reversed(2, 0),
        "the unselected row below is left entirely alone"
    );
}

/// Spec 0194 test-plan item 5 (S1, S3), drawing half. The heat suffix
/// is reachable even though it is drawn past the row's text; the heat
/// glyph in the first column is not, because the caret's screen index
/// is one-based by construction.
#[test]
fn the_caret_reaches_the_heat_suffix_but_never_the_heat_glyph() {
    let mut app = message_node_app();
    let idx = 0;
    let range = extract::message_payload_range(
        &app.blob,
        &app.tree[idx].span.raw_range,
        app.tree[idx].span.packed_record_start,
    );
    super::heat_cue::seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    app.cursor = idx;
    let header = app.absolute_start(idx);

    // `$` — the last reachable column is the suffix's closing bracket.
    let terminal = drawn_frame(&mut app, 60, 8);
    assert!(
        drawn_row(&app, &terminal, 0).ends_with("[10/50]"),
        "the fixture must actually draw a heat suffix: {:?}",
        drawn_row(&app, &terminal, 0)
    );
    app.caret_to_line_end();
    let terminal = drawn_frame(&mut app, 60, 8);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, "]".to_string())]
    );

    // `0` — and the leftmost is the row's first character, never the
    // glyph column the cue itself occupies.
    app.caret_to_line_start();
    let terminal = drawn_frame(&mut app, 60, 8);
    let area = app.main_area;
    assert!(
        !terminal.backend().buffer()[(area.x, area.y)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "column 0 belongs to the heat glyph and is unreachable"
    );
    assert_eq!(caret_cells(&app, &terminal).len(), 1);
}

/// Spec 0194 test-plan item 8 (G6). The caret addresses a character,
/// not a screen column, so nothing that only moves the row sideways may
/// move it: a horizontal pan, or the fold marker appearing beside it.
/// The heat suffix is appended after the pan, so it does not slide
/// either — and the caret standing in it does not slide within it.
#[test]
fn the_caret_holds_its_character_across_a_pan_and_a_fold() {
    let mut app = text_rows_app(&["    abcdefgh"]);
    app.cursor = 0;
    app.cursor_column = 7;
    app.desired_column = 7;
    let mut terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(caret_cells(&app, &terminal), vec![(0, "d".to_string())]);

    app.pan_offset = 3;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(0, "d".to_string())],
        "the pan slides the row, not the caret"
    );

    // The marker appearing beside a row must not shift its text either
    // — spec 0193 reserves the field whether or not it is used.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    app.caret_right();
    let mut terminal = drawn_frame(&mut app, 60, 12);
    let before = caret_cells(&app, &terminal);
    app.toggle_fold(items[1]);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        before,
        "folding the row under the caret does not move it"
    );
}

/// Spec 0194 test-plan item 8, suffix half. The heat suffix is appended
/// after the pan is applied, so it stays put while the text slides
/// underneath it.
#[test]
fn the_heat_suffix_does_not_slide_under_a_pan() {
    let mut app = message_node_app();
    let range = extract::message_payload_range(
        &app.blob,
        &app.tree[0].span.raw_range,
        app.tree[0].span.packed_record_start,
    );
    super::heat_cue::seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    app.cursor = 0;

    let mut terminal = drawn_frame(&mut app, 60, 8);
    let unpanned = drawn_row(&app, &terminal, 0);
    let suffix_at = drawn_column_of(&unpanned, "[10/50]");

    app.caret_to_line_end();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let caret_at_end = caret_cells(&app, &terminal);

    app.pan_offset = 3;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let panned = drawn_row(&app, &terminal, 0);
    assert_eq!(
        drawn_column_of(&panned, "[10/50]"),
        suffix_at - 3,
        "the suffix keeps its place relative to the text it follows"
    );
    assert_ne!(panned, unpanned, "the text itself did pan");
    assert_eq!(
        caret_cells(&app, &terminal),
        caret_at_end,
        "and the caret does not move within the suffix"
    );
}

/// Spec 0194 test-plan item 16 (S1). A row with no text of its own —
/// or none past the fold field — still has one reachable column, drawn
/// on a synthetic trailing space. Without it the caret would simply
/// vanish on such a row.
#[test]
fn the_caret_is_drawn_on_a_synthetic_space_on_a_blank_row() {
    for text in ["", "   "] {
        let mut app = text_rows_app(&[text, "a: 1"]);
        app.cursor = 0;
        app.reset_caret_column();
        let terminal = drawn_frame(&mut app, 40, 6);
        assert_eq!(
            caret_cells(&app, &terminal),
            vec![(0, " ".to_string())],
            "a blank row keeps exactly one reachable column, on {text:?}"
        );
    }
}

/// Spec 0194 test-plan item 18 (A5). The caret's column counts
/// characters, so one press crosses a multi-byte character whole — and
/// the character-index walk that draws it never splits one, which a
/// byte-offset version would.
#[test]
fn the_caret_crosses_a_multi_byte_character_in_one_press() {
    let text = "s: \"héllo\"";
    let mut app = text_rows_app(&[text]);
    app.cursor = 0;
    app.reset_caret_column();

    for want in text.chars() {
        let terminal = drawn_frame(&mut app, 40, 6);
        assert_eq!(
            caret_cells(&app, &terminal),
            vec![(0, want.to_string())],
            "one press per character, including the multi-byte one"
        );
        app.caret_right();
    }
}

/// Spec 0193 test-plan item 10 (S4). vim's own rule, boundaries
/// included — the last full screen must read `Bot` rather than `99%`,
/// which is the case a naive percentage gets wrong.
#[test]
fn viewport_label_matches_vims_rule_at_the_boundaries() {
    for (first, height, total, want) in [
        (0usize, 10usize, 10usize, "All"),
        (0, 10, 3, "All"),
        (0, 10, 100, "Top"),
        (90, 10, 100, "Bot"),
        (89, 10, 100, "98%"),
        (45, 10, 100, "50%"),
        (1, 10, 100, "1%"),
    ] {
        assert_eq!(
            viewport_label(first, height, total),
            want,
            "viewport_label({first}, {height}, {total})"
        );
    }
}

/// Spec 0193 test-plan item 11 (G5) — the reported complaint, end to
/// end. Panning moves the whole screen without moving the cursor, so
/// `L<n>/<m>` is right to sit still; what was missing was anything at
/// all that reported the viewport.
#[test]
fn panning_changes_the_viewport_label_and_not_the_cursor_ruler() {
    let mut app = wide_sibling_scalars_app(200);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    let statusline = |app: &App, terminal: &Terminal<TestBackend>| -> String {
        let buffer = terminal.backend().buffer();
        let y = app.main_area.y + app.main_area.height;
        (app.main_area.x..app.main_area.x + app.main_area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    };

    terminal.draw(|frame| app.render(frame)).unwrap();
    let before = statusline(&app, &terminal);
    assert!(before.ends_with("L1/200  Top"), "at rest: {before:?}");

    let cursor = app.cursor;
    // Five `PAN_STEP`s down: far enough to be neither `Top` nor `Bot`,
    // so the indicator has to answer with a real percentage.
    for _ in 0..5 {
        app.pan_vertical_down();
    }
    terminal.draw(|frame| app.render(frame)).unwrap();
    let after = statusline(&app, &terminal);

    assert_eq!(app.cursor, cursor, "panning must not move the cursor");
    assert!(
        after.ends_with("L1/200  22%"),
        "the cursor ruler stands still and the viewport indicator moves: {after:?}"
    );
}
