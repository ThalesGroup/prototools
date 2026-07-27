// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

#[test]
fn q_confirmation_is_cancelled_by_any_other_key() {
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
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.should_quit);
    assert!(app.quit_confirm);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.should_quit);
    assert!(!app.quit_confirm);
    assert!(app.message.is_empty());

    // A fresh `q` press re-arms confirmation from scratch.
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.quit_confirm);
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.should_quit);
}

/// Spec 0113 D31: `Ctrl-Z` sets `should_suspend` (the actual
/// `SIGTSTP`/terminal dance lives in `run_loop`/`suspend`, outside
/// `App`'s own unit-testable surface) — checked centrally, so it
/// fires uniformly regardless of a pending quit confirmation, and
/// leaves that confirmation untouched.
#[test]
#[cfg(unix)]
fn ctrl_z_sets_should_suspend_without_disturbing_quit_confirm() {
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
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.quit_confirm);

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert!(app.should_suspend);
    assert!(!app.should_quit);
    assert!(
        app.quit_confirm,
        "Ctrl-Z must not disturb a pending quit confirmation"
    );
}

/// The splash screen is transparent to keyboard input (spec 0113 D22
/// amendment): the very first keypress both dismisses it and is
/// processed as a real command, rather than being swallowed.
#[test]
fn splash_dismissing_keypress_is_also_processed_as_a_command() {
    let mut app = message_node_app();
    assert!(app.splash);
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(!app.splash);
    assert_eq!(app.override_target, Some(0));
}

/// `F1` opens the help overlay; `q`, `Esc`, or `F1` closes it — `?` is
/// no longer bound to help, since it now belongs to in-pane search.
#[test]
fn f1_opens_and_closes_the_help_overlay() {
    let mut app = message_node_app();
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.help_open);

    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(!app.help_open);
}

/// Spec 0126 G1: `F1` opens the help overlay regardless of what
/// currently has focus — `manage_focus`, `override_focus`, and an
/// open `command_buffer` all used to swallow it before reaching the
/// main match arm's own `F1` handling.
#[test]
fn f1_opens_help_regardless_of_focus() {
    let mut app = message_node_app();
    app.splash = false;

    app.manage_focus = true;
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.help_open = false;

    app.override_focus = true;
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
    app.help_open = false;
    app.override_focus = false;

    app.command_buffer = Some(String::new());
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.help_open);
}

/// Spec 0185 S5/Q2 (superseding spec 0114 §3's `Tab` toggle): `Tab` no
/// longer leaves the override *selection* pane — focus is locked to it,
/// which is what keeps the preview overlay's `visible_rows` anchor
/// valid. The management pane keeps `Tab`: it splices for real, so its
/// main-pane content is committed content and nothing there depends on
/// an immutable anchor.
#[test]
fn tab_is_locked_out_of_the_override_pane_but_not_the_manage_pane() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.override_focus, "Tab must not leave the selection pane");
    assert_eq!(app.override_target, Some(0), "...and the pane stays open");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_open);
    assert!(app.manage_focus);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!app.manage_focus, "the manage pane does not get the lock");
}

/// Item 3 (spec 0139 follow-up): `Enter` on a main-pane node with an
/// applicable override (active, here — the fixture's seeded root
/// entry) opens the management pane, same as pressing `o` directly.
#[test]
fn enter_opens_the_management_pane_when_an_override_applies() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_open);
    assert!(app.manage_focus);
    assert_eq!(app.override_target, None);
}

/// Item 3 (spec 0139 follow-up): `Enter` on a main-pane node with
/// neither an active nor an applicable-inactive override opens the
/// selection pane instead, same as pressing `t` directly.
#[test]
fn enter_opens_the_override_pane_when_no_override_applies() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.override_focus);
    assert_eq!(app.override_target, Some(0));
    assert!(!app.manage_open);
}

/// Item 14 (2026-07-17 feedback): `Ctrl-Left`/`Ctrl-Right` pan the
/// override pane and the manage pane, mirroring the main pane's own
/// Ctrl-Left/Ctrl-Right (spec 0113 D24) and the mouse's Shift-wheel/
/// native horizontal-scroll pan already available for these panes
/// (`mouse.rs`'s
/// `override_and_manage_panes_pan_independently_of_the_main_pane`).
#[test]
fn ctrl_left_right_pan_the_override_and_manage_panes() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    // 2026-07-19 feedback item 4: pan is now clamped on the right by
    // the widest visible row — set up a candidate list/pane width with
    // plenty of room to pan by a full `PAN_STEP` twice over, so the
    // clamp itself doesn't interfere with this test.
    app.override_candidates = vec![("cand.SomeVeryLongTypeNameHere".to_string(), None)];
    app.override_list_height = 5;
    app.side_area = Rect::new(0, 0, 5, 10);

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.override_pan_offset, PAN_STEP);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.override_pan_offset, 0);

    app.close_override();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.manage_focus);
    app.manage_list_height = 5;
    app.side_area = Rect::new(0, 0, 5, 10);

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.manage_pan_offset, PAN_STEP);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.manage_pan_offset, 0);
}

/// 2026-07-19 feedback items 1/2: `Ctrl-Up`/`Ctrl-Down` pan the override
/// pane's candidate list vertically without moving the highlight,
/// bounded only by the content itself (`0` at the top, `total -
/// list_height` at the bottom) — no longer tied to keeping the
/// highlighted row in view.
#[test]
fn ctrl_up_down_pan_the_override_pane_without_moving_the_highlight() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = (0..30).map(|i| (format!("cand.Type{i}"), None)).collect();
    app.override_list_height = 5;
    app.override_highlight = 19;
    app.override_scroll = 15;
    let max_scroll = 30 - 5; // total candidates - list_height

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll,
        (15 + PAN_STEP).min(max_scroll),
        "Ctrl-Down must pan toward the content's own bottom edge"
    );
    assert_eq!(
        app.override_highlight, 19,
        "panning must not move the highlight"
    );

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll, max_scroll,
        "further Ctrl-Down stops at the content's own bottom edge"
    );

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(app.override_scroll, max_scroll.saturating_sub(PAN_STEP));
    assert_eq!(app.override_highlight, 19);

    app.override_scroll = 0;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(
        app.override_scroll, 0,
        "Ctrl-Up stops at the content's own top edge"
    );
}

/// 2026-07-19 feedback items 1/2: `Ctrl-Up`/`Ctrl-Down` pan the manage
/// pane's list vertically without moving the highlight, bounded only by
/// the content itself (`0` at the top, `total - list_height` at the
/// bottom) — no longer tied to keeping the highlighted row in view.
/// Uses 30 distinct-origin entries (each gets its own `Header` row,
/// spec 0117 §3 amendment).
#[test]
fn ctrl_up_down_pan_the_manage_pane_without_moving_the_highlight() {
    let mut app = message_node_app();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    for field in 1..=30 {
        app.overrides.activate(
            OverrideOrigin::PathField {
                path: "/".to_string(),
                field,
            },
            None,
        );
    }
    let target_field = 15;
    let target_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| {
            e.origin
                == OverrideOrigin::PathField {
                    path: "/".to_string(),
                    field: target_field,
                }
        })
        .unwrap();
    app.manage_highlight = target_idx;
    app.manage_list_height = 5;
    app.manage_scroll = 10;
    let total_rows = app.manage_display_rows().len();
    let max_scroll = total_rows - 5;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll,
        (10 + PAN_STEP).min(max_scroll),
        "Ctrl-Down must pan toward the content's own bottom edge"
    );
    assert_eq!(
        app.manage_highlight, target_idx,
        "panning must not move the highlight"
    );

    app.manage_scroll = max_scroll;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll, max_scroll,
        "further Ctrl-Down stops at the content's own bottom edge"
    );

    app.manage_scroll = 0;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
    assert_eq!(
        app.manage_scroll, 0,
        "Ctrl-Up stops at the content's own top edge"
    );
    assert_eq!(app.manage_highlight, target_idx);
}

/// Spec 0144 G1/G2: `v` resolves the FQDN under focus from whichever
/// pane currently has it — the override candidate pane here — and
/// (with `DescriptorContext::empty_for_test()`'s empty pool) reports
/// G3's "unknown type" outcome.
#[test]
#[cfg(unix)]
fn v_in_override_pane_reports_unknown_type_for_unresolvable_candidate() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = vec![("test.SomeType".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: test.SomeType");
}

/// Spec 0144 G2: the `None` sentinel row (alphabetic-mode row 0) has
/// no declaration to jump to — `v` must not even attempt a lookup.
#[test]
#[cfg(unix)]
fn v_in_override_pane_is_a_no_op_for_the_none_sentinel() {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = vec![("protolens_internal.None".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Spec 0144 G2: the manage pane's highlighted entry's own type is
/// resolved, independent of the override pane/cursor.
#[test]
#[cfg(unix)]
fn v_in_manage_pane_reports_unknown_type_for_the_highlighted_entry() {
    let mut app = message_node_app();
    app.splash = false;
    app.manage_open = true;
    app.manage_focus = true;
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: "/".to_string(),
            field: 1,
        },
        Some("test.ManageType".to_string()),
    );
    app.manage_highlight = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.r#type.as_deref() == Some("test.ManageType"))
        .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: test.ManageType");
}

/// Spec 0144 G2: with neither pane focused, `v` falls back to the
/// main-pane cursor node's own type.
#[test]
#[cfg(unix)]
fn v_in_main_pane_reports_unknown_type_for_the_cursor_node() {
    let mut app = message_node_app();
    app.splash = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "unknown type: google.protobuf.DescriptorProto");
}

/// Spec 0144 G2: a scalar main-pane node carries no `type_fqdn` at
/// all — `v` is a no-op, not a lookup failure, when it also has no
/// active override and no parent schema to fall back to (2026-07-18
/// fix: `fqdn_under_focus` now also consults those, mirroring
/// `status_type_label`, so an enum-typed scalar can resolve — see
/// `v_in_main_pane_resolves_an_enum_scalars_natural_type` below).
#[test]
#[cfg(unix)]
fn v_in_main_pane_is_a_no_op_for_a_scalar_node() {
    let mut app = sibling_leaves_app(&["field: 1"]);
    app.splash = false;
    // This fixture's single node sits at path "/" (root-level, no
    // parent) — drop the seeded root-type override entry first, or
    // it would accidentally match that node and mask the no-op case
    // this test targets.
    while !app.overrides.entries().is_empty() {
        app.overrides.remove(0);
    }

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Regression test (2026-07-18 feedback): `v` on an enum-typed scalar
/// field with no active override must resolve the field's own
/// natural enum type and attempt a lookup — previously
/// `fqdn_under_focus` read only `span.type_fqdn` (always `None` for
/// scalars) for the main-pane branch, so this always reported "no
/// declaration to jump to here" even though `status_type_label`
/// already resolved and displayed the same FQDN on the status line.
#[test]
#[cfg(unix)]
fn v_in_main_pane_resolves_an_enum_scalars_natural_type() {
    let (mut app, durability_idx) = enum_field_fixture();
    app.cursor = durability_idx;
    assert!(app.proto_root.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        "no proto root configured; set one with :proto-root <dir> or -I/--proto-root"
    );
}

/// Spec 0144 G2 (`fqdn_under_focus` doc comment): the internal,
/// non-real `decode::MESSAGE_SET_ITEM_FQDN` placeholder is never
/// registered as a real message — `v` must treat it the same as "no
/// type at all", not surface a confusing "unknown type" message.
#[test]
#[cfg(unix)]
fn v_is_a_no_op_for_the_internal_message_set_item_fqdn() {
    let mut app = message_set_fixture();
    let item_idx = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN))
        .expect("fixture must contain a MessageSet Item node");
    app.cursor = item_idx;

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.message, "no declaration to jump to here");
}

/// Spec 0144 G3/G4: a real, resolving FQDN (`type_as_fixture`'s
/// `test.Inner`) clears G3's own check, but with no `proto_root`
/// configured `v` stops at G4 with a clear message.
#[test]
#[cfg(unix)]
fn v_reports_missing_proto_root_when_type_resolves_but_none_is_configured() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    assert!(app.proto_root.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        "no proto root configured; set one with :proto-root <dir> or -I/--proto-root"
    );
    assert!(app.pending_editor_open.is_none());
}

/// Spec 0144 G4: a configured `proto_root` under which the resolved
/// file doesn't actually exist reports "proto source not found"
/// rather than silently arming the editor handoff.
#[test]
#[cfg(unix)]
fn v_reports_proto_source_not_found_when_the_file_is_missing_under_proto_root() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    let proto_root = std::env::temp_dir().join("protolens-v-test-missing-root");
    app.proto_root = Some(proto_root.clone());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.message,
        format!(
            "proto source not found: test_type_as.proto (under proto-root {})",
            proto_root.display()
        )
    );
    assert!(app.pending_editor_open.is_none());
}

/// Spec 0144 G1-G4: the full happy path — a resolving FQDN, a
/// configured `proto_root`, and the resolved `.proto` file actually
/// present — arms `pending_editor_open` (the fixture carries no
/// `source_code_info`, so `locate_declaration` falls back to line 1,
/// column 1 — see `neovim::locate_declaration`'s own doc comment).
#[test]
#[cfg(unix)]
fn v_arms_pending_editor_open_when_the_proto_source_is_found() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let proto_root = std::env::temp_dir().join(format!("protolens-v-test-root-{n}"));
    std::fs::create_dir_all(&proto_root).unwrap();
    let proto_path = proto_root.join("test_type_as.proto");
    std::fs::write(&proto_path, "").unwrap();
    app.proto_root = Some(proto_root.clone());

    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    std::fs::remove_dir_all(&proto_root).unwrap();

    let req = app
        .pending_editor_open
        .expect("must arm pending_editor_open");
    assert_eq!(req.path, proto_path);
    assert_eq!(req.line, 1);
    assert_eq!(req.col, 1);
}

/// Glitch reported 2026-07-18: a missing `nvim` binary must not crash
/// protolens — the failure is reported via `app.message` and the TUI
/// keeps running. Runtime-probes for a real `nvim` first and skips
/// gracefully instead of assuming this sandbox's `PATH` lacks it — a
/// real `nvim` would otherwise be spawned and block on `waitpid`.
#[test]
fn open_editor_reports_a_missing_nvim_instead_of_crashing() {
    if std::process::Command::new("nvim")
        .arg("--version")
        .output()
        .is_ok()
    {
        eprintln!("skipping: a real nvim is present on PATH");
        return;
    }
    let mut app = empty_app();
    // `open_editor` requires `io::Error: From<B::Error>` (it propagates
    // real I/O errors via `?`) — `TestBackend`'s `Error` is `Infallible`,
    // which doesn't convert, so a `CrosstermBackend` over an in-memory
    // buffer is used instead; it never touches a real terminal.
    let mut terminal = Terminal::new(CrosstermBackend::new(Vec::new())).unwrap();
    let req = neovim::EditorRequest {
        path: PathBuf::from("/tmp/protolens-test-does-not-exist.proto"),
        line: 1,
        col: 1,
    };
    // The return value itself isn't asserted: `enable_raw_mode_and_reenter`
    // (called on the way out, regardless of the branch below) talks to the
    // *real* process stdout/stdin, which isn't a tty under `cargo test`
    // and can legitimately fail here — a pre-existing, orthogonal
    // limitation of this sandboxed test run, not something Glitch 1's fix
    // is responsible for. What this test verifies is that the missing-
    // `nvim` spawn failure itself doesn't propagate as an `Err` (the
    // actual crash reported) but is instead converted to a message.
    let _ = neovim::open_editor(&mut terminal, &mut app, req);
    assert!(app.message.contains("cannot launch nvim"));
    assert!(matches!(app.editor_state, neovim::EditorState::NotRunning));
}

// ── Spec 0156 G3: the `x<b|p|d<b|p>>` export chord ─────────────────────────

/// `x` then `b`/`p` pre-fill `export --binary/--prototext <path>` and
/// open the command line.
#[test]
fn x_b_and_x_p_prefill_export_data_format_and_open_the_command_line() {
    for (key, flag) in [('b', "--binary"), ('p', "--prototext")] {
        let (mut app, _, _) = type_as_fixture();
        let expected_path = app.default_extract_path();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::Leader);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::None);
        assert_eq!(
            app.command_buffer.as_deref(),
            Some(format!("export {flag} {expected_path}").as_str())
        );
        assert_eq!(app.command_kind, CommandLineKind::Command);
    }
}

/// `x` then `d` then `b`/`p` pre-fill `export --descriptor-binary/
/// --descriptor-prototext <path>` and open the command line.
#[test]
fn x_d_b_and_x_d_p_prefill_export_descriptor_format_and_open_the_command_line() {
    for (key, flag) in [
        ('b', "--descriptor-binary"),
        ('p', "--descriptor-prototext"),
    ] {
        let (mut app, _, _) = type_as_fixture();
        let expected_path = app.default_export_descriptor_path();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::Descriptor);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.pending_x, ExportChord::None);
        assert_eq!(
            app.command_buffer.as_deref(),
            Some(format!("export {flag} {expected_path}").as_str())
        );
        assert_eq!(app.command_kind, CommandLineKind::Command);
    }
}

/// `x` then any other key clears `pending_x` back to `None`, does not
/// open the command line, and still performs that key's normal action
/// (cursor moves for `j`).
#[test]
fn x_then_other_key_cancels_and_falls_through_unswallowed() {
    let (mut app, _, _) = type_as_fixture();
    let cursor_before = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Leader);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
    assert_ne!(app.cursor, cursor_before, "the 'j' move must still fire");
}

/// `x` then `d` then any other key clears `pending_x` back to `None`,
/// does not open the command line, and still performs that key's
/// normal action.
#[test]
fn x_d_then_other_key_cancels_and_falls_through_unswallowed() {
    let (mut app, _, _) = type_as_fixture();
    let cursor_before = app.cursor;
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Descriptor);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
    assert_ne!(app.cursor, cursor_before, "the 'j' move must still fire");
}

/// `x` then `x` again: the chord cancels (no pre-fill), and
/// `pending_x` stays `None` afterward — it does not re-arm.
#[test]
fn x_then_x_cancels_without_prefill_and_does_not_rearm() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::Leader);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(app.pending_x, ExportChord::None);
    assert!(app.command_buffer.is_none());
}

/// Plain `x` alone, or `x` then `d` alone (no follow-up key yet):
/// `command_buffer` stays `None`.
#[test]
fn bare_x_and_x_d_alone_leave_the_command_buffer_untouched() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());

    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
}

/// `default_export_descriptor_path` (spec 0156 G5): at the document
/// root, the segment is always `no range`.
#[test]
fn default_export_descriptor_path_uses_no_range_at_the_root() {
    let (app, _, _) = type_as_fixture();
    assert_eq!(app.cursor, app.first_node);
    assert!(app.default_export_descriptor_path().contains(".no range."));
}

/// An active rename override (spec 0119 §G4's `f`) on the cursor node
/// takes priority over both the `no range` root default and the
/// `short_type` suffix: the segment is the renamed field name alone,
/// e.g. `<stem>.exemplar.desc` rather than `<stem>.no range.Outer.desc`.
#[test]
fn default_export_descriptor_path_uses_an_active_rename_over_no_range() {
    let (mut app, _, _) = type_as_fixture();
    assert_eq!(app.cursor, app.first_node);
    app.run_command("type-as test.Outer");
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.active && e.r#type.as_deref() == Some("test.Outer"))
        .expect("type-as on root must have created an active entry");
    app.overrides
        .rename(entry_idx, Some("exemplar".to_string()));
    let path = app.default_export_descriptor_path();
    assert!(
        path.ends_with(".exemplar.desc"),
        "expected the renamed field name to replace `no range.<Type>`, got: {path}"
    );
}

/// A cursor whose parent's schema resolves its field name uses that
/// name as the segment.
#[test]
fn default_export_descriptor_path_uses_the_resolvable_field_name() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.set_cursor(inner_idx);
    assert!(app.default_export_descriptor_path().contains(".inner."));
}

/// A non-root cursor with no resolvable schema field name falls back
/// to the numeric `<start>-<end>` range (same as `default_extract_path`).
#[test]
fn default_export_descriptor_path_falls_back_to_the_numeric_range_when_unresolvable() {
    // Two sibling varint scalar leaves, `parent: None` (so `parent_field`
    // can't resolve a schema field name), each with a real, in-bounds
    // `raw_range` into a real blob (`display_range`'s fallback path
    // parses the wire tag at `raw_range.start`, so an empty/synthetic
    // blob would panic).
    let blob = vec![0x08u8, 0x05, 0x10u8, 0x07];
    let make_node = |field_number: u64,
                     raw_range: std::ops::Range<usize>,
                     next_sibling: Option<usize>,
                     prev_sibling: Option<usize>| TreeNode {
        span: NodeSpan {
            field_number,
            raw_range,
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
        next_sibling,
        prev_sibling,
        doc_next: next_sibling,
        doc_prev: prev_sibling,
        sibling_ordinal: prev_sibling.map_or(1, |p| p as u32 + 2),
        rendered_as: None,
    };
    let tree = vec![
        make_node(1, 0..2, Some(1), None),
        make_node(2, 2..4, None, Some(0)),
    ];
    let decoded = Decoded {
        lines: vec!["a".to_string(), "b".to_string()],
        tree,
        root_type: "google.protobuf.FileDescriptorProto".to_string(),
        blob,
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
    app.set_cursor(1);
    assert_ne!(app.cursor, app.first_node);
    let path = app.default_export_descriptor_path();
    assert!(
        !path.contains("no range") && !path.contains(".inner."),
        "expected a numeric range segment, got: {path}"
    );
}
