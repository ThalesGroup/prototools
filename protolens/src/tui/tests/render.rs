// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

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
        style_hints: Vec::new(),
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
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Char('z'),
        KeyCode::Char('x'),
        KeyCode::End,
    ] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.should_quit);
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.should_quit);
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
    let comment_start = line.find("#@").unwrap();
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
        rendered_as: None,
    };
    let decoded = Decoded {
        lines: vec![line.clone()],
        tree: vec![node],
        root_type: "test.Msg".to_string(),
        blob: vec![0x08, 0x05],
        wrapper_offset: 0,
        style_hints: vec![vec![(comment_start..line.len(), SyntaxRole::Comment)]],
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

    assert!(app.annotations);
    assert_eq!(app.render_line_content(0), line);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(!app.annotations);
    assert_eq!(app.render_line_content(0), "  id: 5");
    let spanned: String = app
        .row_spans(DisplayRow::Committed(0))
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(spanned, "  id: 5");

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(app.annotations);
    assert_eq!(app.render_line_content(0), line);
}

/// Spec 0113 D33: the bold override hint applies to a node's own
/// header and (when it has children) footer line, but must not
/// cascade to descendant lines.
#[test]
fn line_has_active_override_marks_header_and_footer_but_not_children() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("type-as test.Inner");
    assert_eq!(
        app.tree[inner_idx].span.type_fqdn.as_deref(),
        Some("test.Inner")
    );

    let header_line = app.tree[inner_idx].span.text_range.start;
    let footer_line = app.tree[inner_idx].span.text_range.end - 1;
    assert!(app.line_has_active_override(header_line));
    assert!(app.line_has_active_override(footer_line));

    let id_line = app.tree[id_idx].span.text_range.start;
    assert!(!app.line_has_active_override(id_line));
}

/// 2026-07-18 feedback: when the cursor rests on a node's own closing
/// `}` line (spec 0142), the status line's `L<n>` must report the
/// footer line's own number, not the header's.
#[test]
fn status_line_reports_the_footer_line_number_for_a_footer_resting_cursor() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let footer_line = app.tree[inner_idx].span.text_range.end - 1;
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

/// Spec 0147 G2's truncation rule (mirroring vim's `%<`): when the
/// terminal is too narrow for the local statusline's full left-hand
/// content, it is cut short with a trailing `<` marker, while the
/// right-flushed ruler remains shown in full.
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
        row_text.contains('<'),
        "narrow terminal: the left side must be truncated with a marker: {row_text:?}"
    );
    assert!(
        row_text.contains("..") && row_text.trim_end().ends_with(char::is_numeric),
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
