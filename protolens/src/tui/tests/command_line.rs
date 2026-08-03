// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::*;
use super::support::*;

#[test]
fn resolve_command_prefix_and_exact_match() {
    assert_eq!(resolve_command("export"), Ok("export"));
    assert_eq!(resolve_command("e"), Ok("export"));
    assert!(resolve_command("zzz").is_err());
    assert_eq!(resolve_command("override-as"), Ok("override-as"));
    assert_eq!(resolve_command("o"), Ok("override-as"));
}

/// Spec 0236: `:type-as` and `:type-as-raw` are gone — `:override-as`
/// subsumes them exactly (`:type-as <fqdn>` is `:override-as <fqdn>`,
/// `:type-as-raw` is a bare `:override-as`), so keeping them would have
/// meant two spellings of one operation.
#[test]
fn resolve_command_no_longer_knows_type_as() {
    assert!(resolve_command("type-as").is_err());
    assert!(resolve_command("type-as-raw").is_err());
}

/// Spec 0156 G1/G10: the old command names no longer resolve at all
/// (`COMMANDS` no longer lists them), and their replacements do,
/// including unambiguous prefixes.
#[test]
fn resolve_command_reflects_the_0156_renames() {
    assert!(resolve_command("extract").is_err());
    assert!(resolve_command("save-overrides").is_err());
    assert!(resolve_command("restore-overrides").is_err());
    assert_eq!(resolve_command("save"), Ok("save"));
    assert_eq!(resolve_command("restore"), Ok("restore"));
}

/// Item 9 (2026-07-17 feedback): `:quit`, and its unambiguous prefix
/// `:q` (no other command starts with `q`), both quit directly — and
/// since spec 0236 S20 they are the only way to.
#[test]
fn quit_command_and_its_q_prefix_both_resolve_and_quit() {
    assert_eq!(resolve_command("quit"), Ok("quit"));
    assert_eq!(resolve_command("q"), Ok("quit"));

    let mut app = empty_app();
    app.splash = false;
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.should_quit);
}

/// Item 9: `:` opens the command line regardless of which pane
/// currently has keyboard focus — previously unbound (a silent no-op)
/// while the override or manage pane held focus.
#[test]
fn colon_opens_the_command_line_from_override_and_manage_focus() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    for c in "quit".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.should_quit);

    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert!(app.manage_focus);
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    for c in "quit".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.should_quit);
}

#[test]
fn longest_common_prefix_examples() {
    assert_eq!(longest_common_prefix(&["extract", "extra"]), "extra");
    assert_eq!(longest_common_prefix(&["extract"]), "extract");
    assert_eq!(longest_common_prefix(&[]), "");
    assert_eq!(longest_common_prefix(&["abc", "xyz"]), "");
}

/// Spec 0113 D26: `Tab` on a unique-matching command-name prefix
/// completes it in full.
#[test]
fn tab_completes_the_unique_command_name() {
    let mut app = empty_app();
    app.splash = false;
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.command_buffer.as_deref(), Some("export"));
    assert_eq!(app.command_cursor, "export".chars().count());
}

/// Spec 0113 D26: once a space precedes the cursor, `Tab` is a silent
/// no-op for commands with no argument completion — `:export`'s
/// argument is a path it cannot guess. Its *options* do complete
/// (spec 0236 S22), but only once the token begins with `-`.
#[test]
fn tab_is_a_no_op_once_past_the_first_space() {
    let mut app = empty_app();
    app.splash = false;
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "export ".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.command_buffer.as_deref(), Some("export "));
}

/// Spec 0113 D26: repeated `Tab` cycles forward through a multi-
/// candidate list, wrapping around; `Shift-Tab` (`BackTab`) cycles
/// backward. Exercised directly against `handle_tab_key`/a synthetic
/// `CompletionState` (real multi-candidate cycling is also reachable
/// end-to-end via `:override-as`'s FQDN argument — see the
/// `override_as_command_*` tests below).
#[test]
fn tab_cycles_forward_and_shift_tab_cycles_backward() {
    let mut app = empty_app();
    app.command_buffer = Some("xy".to_string());
    app.command_cursor = 2;
    app.completion = Some(CompletionState {
        token_start: 0,
        suffix: String::new(),
        candidates: vec![
            "xyalpha".to_string(),
            "xybeta".to_string(),
            "xygamma".to_string(),
        ],
        index: None,
    });
    app.handle_tab_key(true);
    assert_eq!(app.command_buffer.as_deref(), Some("xyalpha"));
    app.handle_tab_key(true);
    assert_eq!(app.command_buffer.as_deref(), Some("xybeta"));
    app.handle_tab_key(false);
    assert_eq!(app.command_buffer.as_deref(), Some("xyalpha"));
    // Wraps backward past the start.
    app.handle_tab_key(false);
    assert_eq!(app.command_buffer.as_deref(), Some("xygamma"));
}

/// Spec 0125 §G3: `:save` then `:restore`
/// round-trips an `auto: true` entry's `auto` flag exactly, and a
/// pre-existing YAML file with no `auto` key still loads fine
/// (defaults to `false`).
#[test]
fn yaml_round_trips_auto_flag_and_defaults_false_when_absent() {
    let mut collection = override_pane::OverrideCollection::new();
    collection.activate_auto(
        OverrideOrigin::Path {
            path: "/".to_string(),
        },
        Some("pkg.Type".to_string()),
    );
    let yaml = collection.to_yaml("blobsha".to_string(), "descsha".to_string());
    assert!(
        yaml.contains("auto: true"),
        "auto: true must round-trip: {yaml}"
    );
    let (restored, _target) =
        override_pane::OverrideCollection::from_yaml(&yaml).expect("must parse");
    assert!(
        restored.entries()[0].auto,
        "restored entry must keep auto: true"
    );

    // Pre-existing file with no `auto` key at all.
    let legacy_yaml = "version: 1\n\
         target:\n  blob_sha256: blobsha\n  descriptor_set_sha256: descsha\n\
         overrides:\n  - path: /\n    type: pkg.Type\n    active: true\n";
    let (legacy, _target) =
        override_pane::OverrideCollection::from_yaml(legacy_yaml).expect("must parse legacy");
    assert!(
        !legacy.entries()[0].auto,
        "legacy file with no auto key must default to auto: false"
    );
}

/// Quality audit E1: `:save` replaces the target in one step and leaves
/// nothing behind. The old contents surviving intact is the property
/// that matters — `std::fs::write` truncates first, so an interrupted
/// save destroyed a collection that only existed in memory — and the
/// temp file being gone afterward is what makes that affordable.
#[test]
fn write_atomically_replaces_the_target_and_leaves_no_temp_behind() {
    let dir = std::env::temp_dir().join(format!(
        "protolens-atomic-write-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("overrides.yaml");

    command_line::write_atomically(&path, b"first").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    command_line::write_atomically(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");

    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != "overrides.yaml")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );

    // A failing write must not litter either — and must not have
    // touched the target it could not replace.
    let denied = dir.join("no-such-subdir").join("overrides.yaml");
    assert!(command_line::write_atomically(&denied, b"third").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"second");

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Spec 0127 §G1: a long `:command` buffer becomes pannable — typing
/// past the visible width auto-follows the cursor instead of clipping
/// it off-screen, and the same offset can also be panned manually via
/// hover + Shift+wheel/native horizontal scroll.
#[test]
fn long_command_buffer_is_pannable_and_keeps_cursor_visible() {
    let (mut app, _items) = repeated_message_fixture();
    app.splash = false;
    app.command_buffer = Some("a".repeat(80));
    app.command_cursor = 80;

    let backend = TestBackend::new(60, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let cmd_area = app
        .cmd_area
        .expect("command bar must be shown while typing");
    assert!(
        app.command_pan_offset > 0,
        "auto-follow must have panned to keep the cursor visible"
    );
    let cursor_x = cmd_area.x + (1 + app.command_cursor - app.command_pan_offset) as u16;
    assert!(
        cursor_x < cmd_area.x + cmd_area.width,
        "the cursor must stay within the visible command bar"
    );

    let offset_before = app.command_pan_offset;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollLeft,
        column: cmd_area.x,
        row: cmd_area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        app.command_pan_offset < offset_before,
        "hovering + ScrollLeft must pan the command bar left"
    );
}

/// Regression test for a bug found while implementing spec 0123's
/// test plan: `load_overrides` (`:restore`/batch
/// `--load-overrides`) wholesale-replaces `self.overrides` (spec
/// 0117 §4), which used to silently drop the document root's own
/// `seed_root` entry whenever the loaded file didn't carry one
/// itself (the normal case — nobody manually saves a `path: "/"`
/// override). Root's type is external input (CLI `--type`/auto-
/// inference) with no schema-derived fallback (`natural_type` has no
/// parent field descriptor to consult for the root), so losing it
/// reverted root to raw rendering — which cascaded: every ordinary,
/// never-explicitly-overridden descendant's own `natural_type` walks
/// up through its parent's *resolved* type to find its field's
/// schema type, so the whole document (not just root) went raw, and
/// spec 0120's tier-2 MessageSet auto-expansion candidacy gate
/// (which needs its un-overridden grandparent to still resolve as
/// MessageSet-typed) silently stopped firing even when its own
/// override entry was present, correct, and active. Fixed by
/// preserving the currently-resolved root type across the replace
/// when the loaded file doesn't define its own.
#[test]
fn load_overrides_without_a_root_entry_preserves_the_current_root_type() {
    let mut app = message_set_fixture();
    let (blob_sha256, descriptor_set_sha256) = app.target_hashes().unwrap();

    // Deliberately omits any `path: "/"` entry — exactly the shape a
    // real `:save` produces is *not* what's being tested
    // here (that's covered by `save_and_restore_overrides_round_trips
    // _and_drops_unresolvable_entries`); this reproduces a
    // hand-authored/edited file, or any file saved before root ever
    // carried a resolved type.
    let yaml = format!(
        "version: 1\n\
         target:\n\
         \x20 blob_sha256: \"{blob_sha256}\"\n\
         \x20 descriptor_set_sha256: \"{descriptor_set_sha256}\"\n\
         overrides:\n\
         \x20 - path: \"/1/1\"\n\
         \x20   type: protolens_internal.Item\n\
         \x20   active: true\n\
         \x20   name: Item\n\
         \x20 - path: \"/1/1/2\"\n\
         \x20   type: ms_test.ExtPayload\n\
         \x20   active: true\n"
    );
    let file = TempFile::written("load-overrides-no-root.yaml", yaml.as_bytes());
    let warnings = app.load_overrides(file.as_str()).unwrap().warnings;

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        type_name_of(&app, app.first_node) == Some("ms_test.Container"),
        "root must keep its resolved type across a wholesale \
         override-collection replace, even though the loaded file \
         defines no root entry of its own: {:#?}",
        app.document_lines()
    );
    assert!(
        app.document_lines()
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "tier-2 MessageSet auto-expansion must still take effect \
         after --load-overrides, not just tier-1: {:#?}",
        app.document_lines()
    );
}

/// Round-trip regression test (spec 0122 Test Plan item 3 — the
/// original reported bug's exact scenario): decode a MessageSet
/// fixture, let `App::new`'s automatic Any/MessageSet overrides
/// (spec 0120) apply, extract the root as `#@ prototext` text (spec
/// 0123's batch-mode rendering), `encode_text_to_binary` it back to
/// binary, and assert byte-for-byte equality with the original blob.
/// Before spec 0122's fix, `splice_override`'s synthetic `WT_LEN`-only
/// re-decode dropped the MessageSet `Item` group's `#@ group`
/// annotation, so re-encoding lost the group's wire framing entirely.
#[test]
fn round_trip_extract_and_encode_preserves_message_set_group_framing() {
    let app = message_set_fixture();
    let root_idx = app.resolve_path("/").expect("tree must have a root");
    let text = app.extract_bytes(root_idx, ExtractFormat::Text);
    let reencoded = prototext_core::serialize::encode_text::encode_text_to_binary(&text);
    let original = &app.blob[app.wrapper_offset..];
    assert_eq!(
        reencoded, original,
        "round-trip through extract+encode must byte-for-byte match \
         the original blob"
    );
}

/// Spec 0114 §7, now spec 0236: `:override-as <FQDN>` applies the
/// override directly to the cursor node, bypassing the override pane
/// entirely — it must never open (`override_target` stays `None`
/// throughout).
#[test]
fn override_as_command_applies_override_bypassing_pane() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("override-as test.Inner");
    assert!(
        app.override_target.is_none(),
        "the pane must never open for :override-as"
    );
    assert_eq!(type_name_of(&app, inner_idx), Some("test.Inner"));
    assert!(app.message.contains("test.Inner"));
}

/// Spec 0236 S4: a bare `:override-as`, with no `<FQDN>`, marks the
/// cursor node's range as explicitly raw — what `:type-as-raw` did.
#[test]
fn a_bare_override_as_marks_raw() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command("override-as");
    assert!(app.override_target.is_none());
    assert_eq!(app.tree[inner_idx].span.type_fqdn, NO_FQDN);
}

/// Spec 0135 §G4 (test plan item 13): `:override-as sint32` on a
/// `WT_VARINT` node succeeds and renders a zigzag-decoded value;
/// `:override-as float` on the same node — wire-incompatible
/// (`WT_VARINT` vs. `float`'s `WT_I32`) — fails with a clear
/// message-line error, rather than silently applying.
#[test]
fn override_as_command_rejects_a_wire_incompatible_primitive_keyword() {
    let (mut app, _, id_idx) = type_as_fixture();
    app.cursor = id_idx;

    app.run_command("override-as sint32");
    assert!(
        app.message.contains("as sint32"),
        "unexpected message: {}",
        app.message
    );
    let line = &app.document_lines()[app.absolute_start(id_idx)];
    assert!(
        line.contains("sint32"),
        "expected zigzag-decoded sint32 rendering, got: {line:?}"
    );

    app.run_command("override-as float");
    assert!(
        app.message.contains("not wire-compatible"),
        "unexpected message: {}",
        app.message
    );
}

/// Regression test (spec 0135 follow-up, 2026-07-17): deactivating a
/// `:override-as`-created primitive override (via the manage pane's `a`/
/// Space key, i.e. `OverrideCollection::toggle_active`) must actually
/// revert the field's main-pane rendering back to its natural type —
/// not get silently stuck at the last-applied override, which is what
/// happened when `render_overrides`'s child-recursion gate relied
/// solely on `resolve_active_override_entry` (which goes back to
/// `None` the instant the entry is deactivated, orphaning the plain
/// scalar leaf before `resettle_node` could ever run on it again).
#[test]
fn deactivating_a_primitive_override_reverts_the_main_pane_rendering() {
    let (mut app, _, id_idx) = type_as_fixture();
    app.cursor = id_idx;

    app.run_command("override-as sint32");
    let line = app.document_lines()[app.absolute_start(id_idx)].clone();
    assert!(
        line.contains("sint32"),
        "expected zigzag-decoded sint32 rendering, got: {line:?}"
    );

    let id_path = app.positional_path(id_idx);
    let entry_idx = app
        .overrides
        .entries()
        .iter()
        .position(|e| matches!(&e.origin, OverrideOrigin::Path { path } if *path == id_path))
        .expect("override-as must have created an entry for the id field");

    app.overrides.toggle_active(entry_idx);
    app.render_overrides(app.first_node);
    assert!(
        !app.overrides.entries()[entry_idx].active,
        "deactivating must stick across a render pass"
    );
    let line = &app.document_lines()[app.absolute_start(id_idx)];
    assert!(
        !line.contains("sint32"),
        "expected the field to revert to its natural (int32) \
         rendering once the override is deactivated, got: {line:?}"
    );
}

/// Spec 0114 §7, now spec 0236 S14: once the command-name token has
/// unambiguously resolved to `override-as`, `Tab` completes its FQDN
/// argument against `all_type_fqdns`.
#[test]
fn tab_completes_the_override_as_fqdn_argument() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "override-as test.In".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("override-as test.Inner")
    );
}

/// Spec 0236 S22: a token beginning with `-` completes against the
/// command's own option list, the way a shell's completion does —
/// checked before any value completer, since no value the command line
/// accepts starts with `-`.
#[test]
fn tab_completes_an_option_from_a_leading_dash() {
    let (mut app, _, _) = type_as_fixture();
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "override-as --f".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("override-as --field-name")
    );

    // A lone `-` offers every option the command takes; `--o` narrows
    // to the one that matches.
    app.command_buffer = Some("override-as --o".to_string());
    app.command_cursor = "override-as --o".chars().count();
    app.completion = None;
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.command_buffer.as_deref(), Some("override-as --origin"));

    // A command with options of its own completes them too — the
    // dispatch is on the leading `-`, not on the command.
    app.command_buffer = Some("export --desc".to_string());
    app.command_cursor = "export --desc".chars().count();
    app.completion = None;
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("export --descriptor-"),
        "the longest common prefix of the two --descriptor-* options"
    );
}

/// Spec 0184 S1, the trap: ordinals count wire *records*, so the
/// record test must not be `a.packed_record_start ==
/// b.packed_record_start` — two adjacent ordinary scalars both carry
/// `None`, and that comparison would fuse them into a single ordinal,
/// renumbering nearly every path in nearly every document.
///
/// `a` and `b` here are consecutive non-packed scalar siblings and must
/// keep distinct ordinals, `/3` and `/4`.
#[test]
fn two_consecutive_scalar_siblings_keep_distinct_ordinals() {
    let (app, _elems, _tail, a, b) = packed_run_with_tail_fixture();

    let a_path = app.positional_path(a);
    let b_path = app.positional_path(b);
    assert_eq!(a_path, "/3");
    assert_eq!(b_path, "/4");
    assert_ne!(a_path, b_path);
    assert_eq!(app.resolve_path(&a_path), Some(a));
    assert_eq!(app.resolve_path(&b_path), Some(b));
}

/// Spec 0184 S7 had to weaken `resolve_path` from an inverse of
/// `positional_path` to "the node that can be acted upon", because a
/// packed run's N elements shared the record's one path and only the
/// leader could be spliced. Spec 0216 S22 makes the run one node, so
/// there is no longer an element to distinguish from a leader and the
/// plain identity holds again — including for the run.
#[test]
fn resolve_path_round_trips_a_packed_run() {
    let (app, run, tail, _a, _b) = packed_run_with_tail_fixture();

    let run_path = app.positional_path(run);
    assert_eq!(run_path, "/1");
    assert_eq!(app.resolve_path(&run_path), Some(run));
    assert_eq!(app.resolve_path(&app.positional_path(tail)), Some(tail));
}

/// Spec 0117 §4: `resolve_path` is the inverse of `positional_path`
/// for every node reachable from the root, and `None` for a path that
/// doesn't resolve against the current tree. Spec 0184 S7 restates this
/// for packed elements (see `resolve_path_round_trips_a_packed_run`);
/// this fixture has none, so the plain identity holds here.
#[test]
fn resolve_path_is_the_inverse_of_positional_path() {
    let (app, inner_idx, id_idx) = type_as_fixture();
    let outer_idx = app.first_node;
    assert_eq!(
        app.resolve_path(&app.positional_path(outer_idx)),
        Some(outer_idx)
    );
    assert_eq!(
        app.resolve_path(&app.positional_path(inner_idx)),
        Some(inner_idx)
    );
    assert_eq!(app.resolve_path(&app.positional_path(id_idx)), Some(id_idx));
    assert_eq!(app.resolve_path("/99"), None);
}

/// Spec 0188 G1: `resolve_path` starts from `self.first_node` instead
/// of searching the arena for its parentless node, which is only
/// correct while those two are the same node.
///
/// The way that could stop being true is a splice appending a second
/// parentless node, so the case worth checking is a splice on the root
/// itself — retyped to raw and back, which re-decodes the entire
/// document twice. Spec 0216 S12 removes the mechanism rather than the
/// risk: a splice rewrites the slots the arena already has for those
/// bytes, so there is nothing for a second root to be appended to.
#[test]
fn the_root_stays_the_first_node_across_a_root_respice() {
    let mut app = nested_any_fixture();
    let root = app.first_node;
    let origin = OverrideOrigin::Path {
        path: "/".to_string(),
    };

    app.overrides.activate(origin.clone(), None);
    app.render_overrides(root);
    app.overrides
        .activate(origin, Some("acme.Envelope".to_string()));
    app.render_overrides(root);

    assert_eq!(
        (0..app.tree.len())
            .filter(|&i| app.parent(i).is_none())
            .collect::<Vec<_>>(),
        vec![app.first_node],
        "the arena must hold exactly one parentless slot, and it must \
         be `first_node`"
    );
    assert_eq!(app.resolve_path("/"), Some(app.first_node));
}

/// Spec 0117 §4's restore-time validation: `origin_resolves` checks
/// each of the three origin kinds against the current tree/descriptor
/// pool.
#[test]
fn origin_resolves_checks_path_field_and_fqdn_field_origins() {
    let (mut app, inner_idx, _) = type_as_fixture();
    let inner_path = app.positional_path(inner_idx);

    assert!(app.origin_resolves(&OverrideOrigin::Path {
        path: inner_path.clone()
    }));
    assert!(!app.origin_resolves(&OverrideOrigin::Path {
        path: "/99".to_string()
    }));

    assert!(app.origin_resolves(&OverrideOrigin::PathField {
        path: inner_path.clone(),
        field: 1,
    }));
    assert!(!app.origin_resolves(&OverrideOrigin::PathField {
        path: inner_path,
        field: 99,
    }));

    assert!(app.origin_resolves(&OverrideOrigin::FqdnField {
        fqdn: "test.Inner".to_string(),
        field: 1,
    }));
    assert!(!app.origin_resolves(&OverrideOrigin::FqdnField {
        fqdn: "test.Inner".to_string(),
        field: 99,
    }));
    assert!(!app.origin_resolves(&OverrideOrigin::FqdnField {
        fqdn: "test.NoSuchType".to_string(),
        field: 1,
    }));
}

/// Spec 0117 §4: `default_save_overrides_path` mirrors
/// `default_extract_path`'s directory/stem derivation, but always
/// with a `.yaml` extension.
#[test]
fn default_save_overrides_path_uses_blob_stem_with_yaml_extension() {
    let (mut app, _, _) = type_as_fixture();
    app.blob_path = PathBuf::from("/tmp/some/target.pb");
    assert_eq!(app.default_save_overrides_path(), "/tmp/some/target.yaml");
}

/// Spec 0117 §4: `:save`/`:restore` round-trip the collection through
/// YAML, and restore drops an entry whose origin no longer resolves
/// against the current tree — reporting how many it dropped, which is
/// spec 0221 S6's change to what was previously a silent drop.
#[test]
fn save_and_restore_overrides_round_trips_and_drops_unresolvable_entries() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: app.positional_path(inner_idx),
            field: 1,
        },
        Some("test.Inner".to_string()),
    );
    // Doesn't resolve against this tree — must be dropped on restore.
    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/99".to_string(),
        },
        None,
    );
    assert_eq!(app.overrides.entries().len(), 3); // root + the two above

    let file = TempFile::reserved("save-restore.yaml");

    app.run_save_overrides(vec![file.as_str()]);
    assert!(
        app.message.starts_with("saved overrides to"),
        "unexpected message: {}",
        app.message
    );

    app.overrides = override_pane::OverrideCollection::new();
    app.run_restore_overrides(vec![file.as_str()]);

    assert!(
        app.message.starts_with("restored overrides from"),
        "unexpected message: {}",
        app.message
    );
    // Spec 0221 S6: the drop used to be silent, which reads exactly
    // like a file that restored cleanly. It now names the count.
    assert!(
        app.message.contains("1 override(s) dropped"),
        "expected the dropped entry to be reported: {}",
        app.message
    );
    assert_eq!(app.overrides.entries().len(), 2); // "/99" dropped
    assert!(!app
        .overrides
        .entries()
        .iter()
        .any(|e| matches!(&e.origin, OverrideOrigin::Path { path } if path == "/99")));
}

/// Spec 0117 §4: a target-hash mismatch on restore warns in the
/// message line but does not block the restore.
#[test]
fn restore_overrides_warns_on_hash_mismatch_without_blocking() {
    let (mut app, _, _) = type_as_fixture();
    let yaml = app
        .overrides
        .to_yaml("deadbeef".to_string(), "deadbeef".to_string());

    let file = TempFile::written("restore-hash-mismatch.yaml", yaml.as_bytes());

    app.run_restore_overrides(vec![file.as_str()]);

    assert!(app.message.contains("warning"), "{}", app.message);
    assert!(app.message.contains("blob hash mismatch"));
    assert!(app.message.contains("descriptor-set hash mismatch"));
    assert_eq!(app.overrides.entries().len(), 1); // restore still applied
}

/// Spec 0117 §4: `Tab` completes `:save`/`:restore`'s
/// path argument against real directory entries, cycling on the
/// longest common prefix — no `!arg_prefix.contains(' ')` restriction,
/// unlike `:type-as`'s FQDN completer.
#[test]
fn tab_completes_filesystem_path_for_save_argument() {
    let (mut app, _, _) = type_as_fixture();
    let dir =
        std::env::temp_dir().join(format!("protolens-tui-fs-complete-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("alpha.yaml"), b"").unwrap();
    std::fs::write(dir.join("alphabet.yaml"), b"").unwrap();

    let prefix = format!("save {}/al", dir.to_string_lossy());
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in prefix.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let expected = format!("save {}/alpha", dir.to_string_lossy());
    assert_eq!(app.command_buffer.as_deref(), Some(expected.as_str()));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Spec 0144 G4: `:proto-root <dir>` sets `proto_root` when `<dir>` is
/// a real directory; an invalid argument (non-existent, or a file)
/// leaves the previous value untouched.
#[test]
fn proto_root_command_sets_a_valid_directory_and_rejects_an_invalid_one() {
    let mut app = empty_app();
    let dir = std::env::temp_dir().join(format!("protolens-tui-proto-root-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    app.run_command(&format!("proto-root {}", dir.to_string_lossy()));
    assert_eq!(app.proto_root, Some(dir.clone()));
    assert!(app.message.contains("proto-root set to"));

    let missing = dir.join("does-not-exist");
    app.run_command(&format!("proto-root {}", missing.to_string_lossy()));
    assert_eq!(
        app.proto_root,
        Some(dir.clone()),
        "an invalid directory must leave the previous value untouched"
    );
    assert!(app.message.starts_with("not a directory:"));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Spec 0144 G4: `Tab` completes `:proto-root`'s directory argument
/// against real directory entries only — a file with a matching
/// prefix must not be offered as a candidate.
#[test]
fn tab_completes_proto_root_directory_argument_excluding_files() {
    let mut app = empty_app();
    let dir = std::env::temp_dir().join(format!(
        "protolens-tui-proto-root-complete-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("alpha_dir")).unwrap();
    std::fs::write(dir.join("alpha_file.proto"), b"").unwrap();

    let prefix = format!("proto-root {}/al", dir.to_string_lossy());
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in prefix.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let expected = format!("proto-root {}/alpha_dir/", dir.to_string_lossy());
    assert_eq!(app.command_buffer.as_deref(), Some(expected.as_str()));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Ctrl-f/Ctrl-b move the cursor one char forward/backward, same as
/// the plain arrow keys, and clamp at the buffer's ends.
#[test]
fn ctrl_f_and_ctrl_b_move_cursor_by_one_char() {
    let mut app = empty_app();
    app.command_buffer = Some("abc".to_string());
    app.command_cursor = 1;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 2);
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 3);
    // Clamped at the end.
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 3);

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 2);
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    // Clamped at the start.
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 0);
}

/// Alt-b/Alt-Left move the cursor to the start of the previous
/// whitespace-delimited word; Alt-f/Alt-Right move it to the end of
/// the next one — mirroring vim/readline command-line word motion.
#[test]
fn alt_word_motion_moves_by_whitespace_delimited_words() {
    let mut app = empty_app();
    app.command_buffer = Some("save foo bar".to_string());
    app.command_cursor = 12; // end of buffer, right after "bar"

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 9); // start of "bar"
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 5); // start of "foo"
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 0); // start of "save"
                                       // Clamped at the start.
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 4); // end of "save"
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 8); // end of "foo"
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 12); // end of "bar"
                                        // Clamped at the end.
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(app.command_cursor, 12);
}

/// Glitch reported 2026-07-18: completing while the cursor sits right
/// before an already-present `/` (e.g. after Left-arrow-ing back into an
/// earlier path segment) must not double up the separator.
#[test]
fn tab_completion_does_not_double_a_slash_when_cursor_precedes_one() {
    let mut app = empty_app();
    let dir = std::env::temp_dir().join(format!(
        "protolens-tui-proto-root-noslash-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("alpha")).unwrap();

    let prefix = format!("proto-root {}/alpha/", dir.to_string_lossy());
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in prefix.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Cursor is now right after the trailing `/`; move it back one
    // position so it sits right before that same `/` instead.
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let expected = format!("proto-root {}/alpha/", dir.to_string_lossy());
    assert_eq!(app.command_buffer.as_deref(), Some(expected.as_str()));

    std::fs::remove_dir_all(&dir).unwrap();
}
