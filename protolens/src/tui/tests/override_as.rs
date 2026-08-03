// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0236: `:override-as`, the one command that sets an override
//! entry's type, origin and display name together.

use super::super::override_as::parse_override_as;
use super::super::*;
use super::support::*;
use crate::override_pane::OverrideEntry;

/// The *active* entry for `origin`, whatever its position in the
/// (re-sorted on every edit) collection. Active, not merely matching:
/// re-typing an origin leaves the previous type behind as a
/// deactivated entry with the same origin (spec 0117 §1), and that one
/// sorts first when its type is `None`.
fn entry_at(app: &App, origin: &OverrideOrigin) -> OverrideEntry {
    app.overrides
        .entries()
        .iter()
        .find(|e| &e.origin == origin && e.active)
        .unwrap_or_else(|| panic!("no active entry for {}", origin.label()))
        .clone()
}

/// Spec 0236 G1: one command, all three dimensions at once — the whole
/// point, since before this each needed a different mechanism and the
/// origin needed the entry deleted and rebuilt.
#[test]
fn override_as_sets_type_origin_and_name_together() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;

    app.run_command("override-as test.Inner --origin test.Outer:1 --field-name payload");

    let origin = OverrideOrigin::FqdnField {
        fqdn: "test.Outer".to_string(),
        field: 1,
    };
    let entry = entry_at(&app, &origin);
    assert!(entry.active);
    assert_eq!(entry.r#type.as_deref(), Some("test.Inner"));
    assert_eq!(entry.name.as_deref(), Some("payload"));
    assert_eq!(app.field_name_for(inner_idx), "payload");
}

/// Spec 0236 S4: absent means default, uniformly. A bare
/// `:override-as` is what `:type-as-raw` was, and each flag omitted
/// independently falls back on its own — which is what makes deleting
/// an argument from the pre-filled line a way to ask for its default.
#[test]
fn override_as_absent_arguments_take_their_defaults() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let cursor_origin = OverrideOrigin::Path {
        path: app.positional_path(inner_idx),
    };

    // No type at all: raw, on the cursor node's own `Path` origin.
    app.run_command("override-as");
    let entry = entry_at(&app, &cursor_origin);
    assert!(entry.r#type.is_none(), "an absent type means raw");
    assert!(entry.name.is_none(), "an absent --field-name means no name");
    assert_eq!(app.tree[inner_idx].span.type_fqdn, NO_FQDN);

    // A type, still no origin: the cursor node again.
    app.run_command("override-as test.Inner");
    assert_eq!(
        entry_at(&app, &cursor_origin).r#type.as_deref(),
        Some("test.Inner")
    );

    // An origin, no type: raw at that origin.
    app.run_command("override-as --origin test.Outer:1");
    let origin = OverrideOrigin::FqdnField {
        fqdn: "test.Outer".to_string(),
        field: 1,
    };
    assert!(entry_at(&app, &origin).r#type.is_none());
}

/// Spec 0236 S5: `<origin>` parses by shape, the inverse of
/// `OverrideOrigin::label`. The split is on the *last* colon, and the
/// container is an FQDN whenever it does not start with `/` — a
/// top-level message in no package has a legal, undotted FQDN.
#[test]
fn override_as_parses_all_three_origin_shapes() {
    let origin = |arg: &str| {
        parse_override_as(&["--origin", arg])
            .unwrap_or_else(|e| panic!("{arg} must parse: {e}"))
            .origin
            .expect("--origin was given")
    };

    assert_eq!(
        origin("/1/2"),
        OverrideOrigin::Path {
            path: "/1/2".to_string()
        }
    );
    assert_eq!(
        origin("/1:7"),
        OverrideOrigin::PathField {
            path: "/1".to_string(),
            field: 7
        }
    );
    assert_eq!(
        origin("pkg.Msg:7"),
        OverrideOrigin::FqdnField {
            fqdn: "pkg.Msg".to_string(),
            field: 7
        }
    );
    assert_eq!(
        origin("Msg:7"),
        OverrideOrigin::FqdnField {
            fqdn: "Msg".to_string(),
            field: 7
        },
        "an undotted FQDN is still an FQDN"
    );

    // Every rejection names the three shapes rather than just refusing.
    for bad in ["nocolon", "pkg.Msg:x", ":7"] {
        let err = parse_override_as(&["--origin", bad]).expect_err("must be rejected");
        assert!(
            err.contains("expected /path, /path:field, or fqdn:field"),
            "the error must teach the grammar: {err}"
        );
    }

    assert!(parse_override_as(&["--origin"])
        .expect_err("a flag with no value is an error")
        .contains("--origin needs a value"));
    assert!(parse_override_as(&["--nope"])
        .expect_err("an unknown flag is an error")
        .contains("unknown flag"));
    assert!(parse_override_as(&["a", "b"])
        .expect_err("only one positional")
        .contains("second type"));
}

/// Spec 0236 G2/S8: `o` pre-fills what is already true, so `Enter` on
/// the unedited line changes nothing. The load-bearing half is S8's
/// normalization — the pre-filled `--field-name` is the *schema*-derived
/// name here, and storing it would write a redundant name override into
/// the entry and into the saved YAML.
#[test]
fn o_prefills_the_current_state_and_enter_is_a_no_op() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.run_command("override-as test.Inner");
    let before = app.overrides.entries().to_vec();

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert_eq!(
        buf,
        format!(
            "override-as test.Inner --origin {} --field-name inner",
            app.positional_path(inner_idx)
        ),
        "every argument present, none elided"
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries(),
        before.as_slice(),
        "o then Enter must change nothing"
    );
}

/// Spec 0236 S8's last fallback: with no entry name and no schema to
/// ask, `--field-name` pre-fills `f<P>` where `<P>` is the node's
/// position among its siblings — *not* its field number. The group
/// fixture is what tells the two apart: `grp` is field 5 and the first
/// child.
#[test]
fn o_prefills_f_position_when_the_schema_names_nothing() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;

    // Strip the schema: with the root raw, `grp`'s parent declares no
    // field for it and `schema_field_name` has nothing to return.
    app.cursor = app.first_node;
    app.run_command("override-as");

    app.cursor = grp_idx;
    assert_eq!(app.sibling_position(grp_idx), 1);
    assert_eq!(
        app.tree[grp_idx].span.field_number, 5,
        "the fixture must keep position and field number distinct"
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert!(
        buf.ends_with("--field-name f1"),
        "expected the f<position> fallback, got: {buf}"
    );
}

/// Spec 0236 S9: the success message reports the origin's blast
/// radius. This is what makes a re-scope safe to perform blind —
/// widening `PathField` to `FqdnField` silently takes an override from
/// one node to every occurrence of that field, and the count is the
/// only place that widening is visible.
#[test]
fn override_as_reports_the_affected_node_count() {
    let (mut app, items) = repeated_message_fixture();
    app.cursor = items[0];

    app.run_command("override-as --origin test.Outer:1");
    assert!(
        app.message.contains("test.Outer:1 as raw — 3 nodes"),
        "unexpected message: {}",
        app.message
    );

    app.run_command(&format!(
        "override-as --origin {}",
        app.positional_path(items[0])
    ));
    assert!(
        app.message.ends_with("as raw — 1 node"),
        "a Path origin affects exactly its own node: {}",
        app.message
    );
}

/// Spec 0236 S10: (origin, type) is an entry's identity, so an edit
/// that lands on an existing entry merges into it rather than
/// duplicating it — the only non-destructive answer, since the
/// alternative is two entries fighting over one origin.
#[test]
fn override_as_merges_into_an_existing_entry() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;

    app.run_command("override-as test.Inner --origin test.Outer:1 --field-name first");
    let count = app.overrides.entries().len();

    app.run_command("override-as test.Inner --origin test.Outer:1 --field-name second");
    assert_eq!(
        app.overrides.entries().len(),
        count,
        "the same origin and type must reuse the entry"
    );
    let entry = entry_at(
        &app,
        &OverrideOrigin::FqdnField {
            fqdn: "test.Outer".to_string(),
            field: 1,
        },
    );
    assert_eq!(entry.name.as_deref(), Some("second"));
}

/// Spec 0236 S13/S14: completion dispatches on the token being
/// completed, not on a fixed argument position — the flags may come in
/// either order. `--origin` offers only the origins `origin_for_kind`
/// can actually build here, which is what keeps re-scoping honest: the
/// user Tabs through what works instead of learning the constraint from
/// an error.
#[test]
fn override_as_completes_types_and_origins() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;

    let complete = |app: &mut App, line: &str| {
        app.command_buffer = Some(line.to_string());
        app.command_cursor = line.chars().count();
        app.completion = None;
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.command_buffer.clone().unwrap_or_default()
    };

    assert_eq!(
        complete(&mut app, "override-as test.In"),
        "override-as test.Inner"
    );

    // The origins the cursor node can be re-scoped onto, in the order
    // `--origin` offers them. All three are buildable here: `inner` has
    // a parent, and that parent's type resolves.
    let expected: Vec<String> = [
        OverrideKind::Path,
        OverrideKind::PathField,
        OverrideKind::FqdnField,
    ]
    .into_iter()
    .map(|kind| {
        app.origin_for_kind(inner_idx, kind)
            .expect("all three kinds are buildable for inner")
            .label()
    })
    .collect();

    complete(&mut app, "override-as --origin ");
    let offered: Vec<String> = app
        .completion
        .as_ref()
        .expect("--origin must offer candidates")
        .candidates
        .clone();
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(offered, sorted);

    // The positional still completes as a type after a flag has been
    // given — the dispatch is on the token, not the position.
    assert_eq!(
        complete(&mut app, "override-as --field-name x test.In"),
        "override-as --field-name x test.Inner"
    );

    // And `--field-name`'s own value completes to nothing: a display
    // name has no candidate list to draw on.
    assert_eq!(
        complete(&mut app, "override-as --field-name zz"),
        "override-as --field-name zz"
    );
}
