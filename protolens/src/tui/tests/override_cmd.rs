// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Specs 0236/0237: `:override`, the one command that sets an override
//! entry's origin, type and display name together.

use super::super::heat_worker::RangeHeatEntry;
use super::super::override_cmd::parse_override;
use super::super::tiered::Tier;
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

/// Put `line` on the command line, press Tab, and return what the line
/// became. `completion = None` first, so each call starts a fresh
/// completion rather than cycling the previous one's candidates.
fn complete(app: &mut App, line: &str) -> String {
    app.command_buffer = Some(line.to_string());
    app.command_cursor = line.chars().count();
    app.completion = None;
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.command_buffer.clone().unwrap_or_default()
}

/// Press Tab again, continuing whatever rotation is in flight.
fn tab_again(app: &mut App) -> String {
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.command_buffer.clone().unwrap_or_default()
}

/// Spec 0236 G1 / 0237 G1: one command, all three dimensions at once —
/// the whole point, since before this each needed a different mechanism
/// and the origin needed the entry deleted and rebuilt. Spec 0237 puts
/// the origin in the positional slot, since it is what an override *is*.
#[test]
fn override_sets_origin_type_and_name_together() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;

    app.run_command("override test.Outer:1 --as test.Inner --field-name payload");

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

/// Spec 0237 S2: `<origin>` is the one argument with no sensible
/// default now that it is what the command is about, so a bare
/// `:override` is an error — one that names the three shapes rather
/// than just refusing.
#[test]
fn override_requires_an_origin() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let before = app.overrides.entries().to_vec();

    app.run_command("override --as test.Inner");
    assert!(
        app.message.contains("missing <origin>")
            && app
                .message
                .contains("expected /path, /path:field, or fqdn:field"),
        "unexpected message: {}",
        app.message
    );
    assert_eq!(
        app.overrides.entries(),
        before.as_slice(),
        "a rejected line must record nothing"
    );

    app.run_command(&format!("override {}", app.positional_path(inner_idx)));
    assert!(app.message.ends_with("as raw — 1 node"));
}

/// Spec 0237 S3: the type moved to `--as`, and absent still means raw.
/// Spec 0236 S4's "absent means default" survives for both flags, which
/// is what makes deleting an argument from the pre-filled line a way to
/// ask for its default.
#[test]
fn override_takes_its_type_from_the_as_flag() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path_origin = OverrideOrigin::Path {
        path: app.positional_path(inner_idx),
    };
    let path = app.positional_path(inner_idx);

    app.run_command(&format!("override {path} --as test.Inner"));
    let entry = entry_at(&app, &path_origin);
    assert_eq!(entry.r#type.as_deref(), Some("test.Inner"));
    assert!(entry.name.is_none(), "an absent --field-name means no name");

    app.run_command(&format!("override {path}"));
    assert!(
        entry_at(&app, &path_origin).r#type.is_none(),
        "an absent --as means raw"
    );
    assert_eq!(app.tree[inner_idx].span.type_fqdn, NO_FQDN);
}

/// Spec 0236 S5: `<origin>` parses by shape, the inverse of
/// `OverrideOrigin::label`. The split is on the *last* colon, and the
/// container is an FQDN whenever it does not start with `/` — a
/// top-level message in no package has a legal, undotted FQDN.
#[test]
fn override_parses_all_three_origin_shapes() {
    let origin = |arg: &str| {
        parse_override(&[arg])
            .unwrap_or_else(|e| panic!("{arg} must parse: {e}"))
            .origin
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
        let err = parse_override(&[bad]).expect_err("must be rejected");
        assert!(
            err.contains("expected /path, /path:field, or fqdn:field"),
            "the error must teach the grammar: {err}"
        );
    }

    assert!(parse_override(&["/1", "--as"])
        .expect_err("a flag with no value is an error")
        .contains("--as needs a value"));
    assert!(parse_override(&["/1", "--field-name"])
        .expect_err("a flag with no value is an error")
        .contains("--field-name needs a value"));
    assert!(parse_override(&["/1", "--nope"])
        .expect_err("an unknown flag is an error")
        .contains("unknown flag"));
    assert!(parse_override(&["/1", "/2"])
        .expect_err("only one positional")
        .contains("second origin"));
}

/// Spec 0236 G2/S8: `o` pre-fills what is already true, so `Enter` on
/// the unedited line changes nothing. The load-bearing half is S8's
/// normalization — the pre-filled `--field-name` is the *schema*-derived
/// name here, and storing it would write a redundant name override into
/// the entry and into the saved YAML.
///
/// Driven from the management pane, one of the two panes `o` edits from
/// (spec 0236 S19): in the main pane `o` opens that pane instead.
#[test]
fn o_prefills_the_current_state_and_enter_is_a_no_op() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);
    app.run_command(&format!("override {path} --as test.Inner"));
    let before = app.overrides.entries().to_vec();

    // The first `o` opens the management pane on that one entry; the
    // second edits it.
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert!(app.manage_open && app.manage_focus);
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert_eq!(
        buf,
        "override {path} --as test.Inner --field-name inner".replace("{path}", &path),
        "every argument present, none elided"
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries(),
        before.as_slice(),
        "o then Enter must change nothing"
    );
}

/// Spec 0321 S1: in the selection pane the pre-filled origin is the one
/// the status line is projecting — spec 0308's widest-first ladder here,
/// since nothing covers the node — and it follows `z` for the same
/// reason `Enter` does. The two exits from one pane must not describe
/// the same subject differently.
#[test]
fn o_in_the_selection_pane_prefills_the_projected_origin() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.set_cursor(inner_idx);

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let projected = app
        .projected_override_origin()
        .expect("the ladder builds an origin for inner");
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert!(
        buf.starts_with(&format!("override {} ", projected.label())),
        "the projected origin, not the node's bare path: {buf}"
    );
    assert_ne!(
        projected,
        OverrideOrigin::Path {
            path: app.positional_path(inner_idx)
        },
        "the ladder must actually have widened, or this proves nothing",
    );

    // `z` moves the projection, so it must move the pre-fill with it.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    let pinned = app
        .projected_override_origin()
        .expect("the pinned kind still builds");
    assert_ne!(pinned, projected, "z must have moved the projection");
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert!(
        buf.starts_with(&format!("override {} ", pinned.label())),
        "the pre-fill follows the pin: {buf}"
    );
}

/// Spec 0321 S2: the command answers the question the selection pane was
/// asking, so committing it closes the pane. The refusal path is the
/// other half — it has answered nothing, so the pane stays up.
#[test]
fn committing_override_closes_the_selection_pane() {
    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.splash = false;
    app.set_cursor(inner_idx);
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_target.is_some());

    let path = app.positional_path(inner_idx);
    app.run_command(&format!("override {path} --as test.Inner"));
    assert!(
        app.override_target.is_none() && !app.override_focus,
        "the pane must be gone",
    );
    assert!(
        app.overrides
            .entries()
            .iter()
            .any(|e| e.active && e.r#type.as_deref() == Some("test.Inner")),
        "and the entry must exist",
    );

    // A refused command leaves the pane exactly as it was. `id` is a
    // varint field, so a `message` keyword is wire-incompatible with it.
    app.set_cursor(id_idx);
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let target = app.override_target;
    app.run_command(&format!(
        "override {} --as message",
        app.positional_path(id_idx)
    ));
    assert_eq!(app.override_target, target, "the pane must still be up");
    assert!(
        app.message.contains("wire"),
        "and the refusal must be visible: {}",
        app.message
    );
}

/// Spec 0237 S4, as narrowed by spec 0321 N2: the pre-filled origin is
/// still the applicable entry's whenever that entry is the widest shape
/// the ladder can build — which an `fqdn:field` entry is. So
/// `o`-then-`Enter` on a covered node remains the no-op spec 0236 G2
/// promises, rather than silently narrowing the entry to this one node.
#[test]
fn o_prefills_the_applicable_entry_origin() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.run_command("override test.Outer:1 --as test.Inner");
    let before = app.overrides.entries().to_vec();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let buf = app.command_buffer.clone().expect("o must open the line");
    assert!(
        buf.starts_with("override test.Outer:1 "),
        "the applicable entry's own origin, not the node's path: {buf}"
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.overrides.entries(),
        before.as_slice(),
        "the entry's blast radius must be unchanged"
    );
}

/// Spec 0237 S6: `<origin>` completion is an unfiltered rotation
/// through the buildable shapes, narrowest first — the order in which a
/// user widens a scope. Unfiltered because the token is almost always
/// the pre-filled origin, which prefix-matches none of the others.
#[test]
fn origin_completion_rotates_narrowest_first() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;

    let label = |app: &App, kind| {
        app.origin_for_kind(inner_idx, kind)
            .expect("all three kinds are buildable for inner")
            .label()
    };
    let path = label(&app, OverrideKind::Path);
    let path_field = label(&app, OverrideKind::PathField);
    let fqdn_field = label(&app, OverrideKind::FqdnField);

    // Starting from the pre-filled (narrowest) origin, the first Tab
    // must already move — an `apply_completion`-style first press would
    // only prime the cycle and appear to do nothing.
    assert_eq!(
        complete(&mut app, &format!("override {path}")),
        format!("override {path_field}")
    );
    assert_eq!(tab_again(&mut app), format!("override {fqdn_field}"));
    assert_eq!(tab_again(&mut app), format!("override {path}"), "wraps");
}

/// The other half of S6: shapes `origin_for_kind` cannot build are
/// skipped rather than offered and then refused. `fqdn:field` needs the
/// parent's resolved type FQDN, which a raw parent does not have.
#[test]
fn origin_completion_skips_unbuildable_shapes() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    let root = app.first_node;
    app.run_command(&format!("override {}", app.positional_path(root)));
    app.cursor = grp_idx;

    let path = app.positional_path(grp_idx);
    let path_field = app
        .origin_for_kind(grp_idx, OverrideKind::PathField)
        .unwrap()
        .label();
    assert!(app
        .origin_for_kind(grp_idx, OverrideKind::FqdnField)
        .is_err());

    assert_eq!(
        complete(&mut app, &format!("override {path}")),
        format!("override {path_field}")
    );
    assert_eq!(
        tab_again(&mut app),
        format!("override {path}"),
        "two shapes, so the rotation wraps after the second"
    );
}

/// Spec 0237 S7: `--field-name` rotates the four derivations of a
/// field's display name. The group fixture is what tells (3) and (4)
/// apart — `grp` is field 5 and the first child — which is exactly the
/// defect spec 0236's single `f<position>` candidate had.
#[test]
fn field_name_completion_rotates_four_derivations() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.cursor = grp_idx;
    assert_eq!(app.sibling_position(grp_idx), 1);
    assert_eq!(
        app.tree[grp_idx].span.field_number, 5,
        "the fixture must keep position and field number distinct"
    );

    let path = app.positional_path(grp_idx);
    app.run_command(&format!("override {path} --field-name custom"));

    let line = format!("override {path} --field-name ");
    assert_eq!(complete(&mut app, &line), format!("{line}custom"));
    assert_eq!(tab_again(&mut app), format!("{line}grp"));
    assert_eq!(tab_again(&mut app), format!("{line}f5"));
    assert_eq!(tab_again(&mut app), format!("{line}p1"));
    assert_eq!(tab_again(&mut app), format!("{line}custom"), "wraps");
}

/// The rest of S7: duplicates are dropped, keeping the first. (1) and
/// (2) coincide whenever the stored name came from the schema, and
/// without the dedup Tab there would appear to do nothing.
#[test]
fn field_name_completion_drops_duplicate_derivations() {
    let (mut app, grp_idx) = group_type_fixture();
    app.splash = false;
    app.cursor = grp_idx;

    let path = app.positional_path(grp_idx);
    app.run_command(&format!("override {path}"));
    // Spec 0236 S8 normalizes a schema-equal `--field-name` away, so the
    // duplicate has to be installed behind the command's back.
    let entry = app.resolve_active_override_entry_index(grp_idx).unwrap();
    app.overrides.rename(entry, Some("grp".to_string()));

    let line = format!("override {path} --field-name ");
    assert_eq!(complete(&mut app, &line), format!("{line}grp"));
    assert_eq!(tab_again(&mut app), format!("{line}f5"));
    assert_eq!(tab_again(&mut app), format!("{line}p1"));
    assert_eq!(
        tab_again(&mut app),
        format!("{line}grp"),
        "three derivations, not four"
    );
}

/// Spec 0237 S8: `--as` offers the inferred candidates in decreasing
/// score order. The two lists are *sequenced*, never concatenated — a
/// prefix that matches an inferred type must not also drag in the
/// unranked FQDNs sharing it, which is the whole value of ranking them.
#[test]
fn as_completion_prefers_inferred_order() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    app.override_list_height = 2;

    let range = app.heat_scored_range(inner_idx);
    app.heat_caches.lock().unwrap().by_range.upsert(
        range.start,
        RangeHeatEntry {
            best_score: Some(90),
            best_count: 1,
            top_n: vec![
                ("test.Outer".to_string(), 90),
                ("test.Inner".to_string(), 40),
            ],
        },
        Tier::User,
    );

    let path = app.positional_path(inner_idx);
    let line = format!("override {path} --as ");
    complete(&mut app, &format!("{line}test."));
    let offered = app
        .completion
        .as_ref()
        .expect("two inferred candidates must cycle")
        .candidates
        .clone();
    assert_eq!(
        offered,
        vec!["test.Outer".to_string(), "test.Inner".to_string()],
        "decreasing score, not alphabetical"
    );

    // A prefix no inferred type matches falls through to the
    // lexicographic list — and the ranked names do not come with it.
    assert_eq!(
        complete(&mut app, &format!("{line}str")),
        format!("{line}string")
    );
}

/// Spec 0237 S8/N2: a cold cache queues the scoring request and yields
/// nothing, so completion answers from the lexicographic list — and
/// says nothing about it. A completer that sometimes ignores a
/// keystroke is worse than one whose order is sometimes alphabetical.
#[test]
fn as_completion_falls_back_on_a_cold_cache() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;

    let path = app.positional_path(inner_idx);
    let line = format!("override {path} --as ");
    assert_eq!(
        complete(&mut app, &format!("{line}test.In")),
        format!("{line}test.Inner")
    );
    assert!(app.message.is_empty(), "silently, per N2: {}", app.message);
}

/// Spec 0236 S9: the success message reports the origin's blast
/// radius. This is what makes a re-scope safe to perform blind —
/// widening `PathField` to `FqdnField` silently takes an override from
/// one node to every occurrence of that field, and the count is the
/// only place that widening is visible.
#[test]
fn override_reports_the_affected_node_count() {
    let (mut app, items) = repeated_message_fixture();
    app.cursor = items[0];

    app.run_command("override test.Outer:1");
    assert!(
        app.message.contains("test.Outer:1 as raw — 3 nodes"),
        "unexpected message: {}",
        app.message
    );

    app.run_command(&format!("override {}", app.positional_path(items[0])));
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
fn override_merges_into_an_existing_entry() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;

    app.run_command("override test.Outer:1 --as test.Inner --field-name first");
    let count = app.overrides.entries().len();

    app.run_command("override test.Outer:1 --as test.Inner --field-name second");
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
/// either order, before or after the positional.
#[test]
fn override_completion_dispatches_on_the_token() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);

    assert_eq!(
        complete(&mut app, &format!("override --as test.In {path}")),
        format!(
            "override --as test.In {}",
            app.origin_for_kind(inner_idx, OverrideKind::PathField)
                .unwrap()
                .label()
        ),
        "the trailing token is the origin, whatever precedes it"
    );
    assert_eq!(
        complete(
            &mut app,
            &format!("override {path} --field-name x --as test.In")
        ),
        format!("override {path} --field-name x --as test.Inner")
    );
}

// ── spec 0315: `--as-new`, the reader's own type name ────────────────

/// `node`'s rendered subtree, minus its header row — the row that
/// carries the `#@` type annotation, and hence the one row a declared
/// type is *supposed* to spell differently from `message`.
fn subtree_body(app: &App, node: usize) -> Vec<String> {
    let start = app.absolute_start(node);
    (1..app.tree[node].lines_total)
        .map(|k| {
            let pos = app
                .line_pos(start + k as usize)
                .expect("line is inside the subtree");
            app.line_text(pos).into_owned()
        })
        .collect()
}

/// Spec 0315 G1/S1/S2: `--as-new` declares a type the descriptor set
/// does not have and applies it in the same breath, and the entry it
/// stores is indistinguishable from an `--as` one — declaring is a
/// property of the invocation.
///
/// The rendering assertion is the substance: a declared type is spec
/// 0299's zero-field message under the reader's own name, so the bytes
/// under it read exactly as they do under `--as message`. Compared
/// against `message`'s own render rather than a hand-copied
/// expectation, so the two cannot drift.
#[test]
fn as_new_declares_a_type_the_pool_did_not_have() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);

    app.run_command(&format!("override {path} --as message"));
    let under_keyword = subtree_body(&app, inner_idx);

    app.run_command(&format!("override {path} --as-new mine.Foo"));

    let entry = entry_at(&app, &OverrideOrigin::Path { path: path.clone() });
    assert_eq!(
        entry.r#type.as_deref(),
        Some("mine.Foo"),
        "the entry stores the bare FQDN, exactly as `--as mine.Foo` would"
    );
    assert_eq!(type_name_of(&app, inner_idx), Some("mine.Foo"));
    assert_eq!(app.ctx.created_types(), ["mine.Foo".to_string()]);
    assert_eq!(
        subtree_body(&app, inner_idx),
        under_keyword,
        "a declared type is `message` with a name on it"
    );
}

/// Spec 0315 G3/S3: re-declaring is not an error. It is what lets a
/// scripted step (spec 0271) be stepped over twice, and it is safe
/// because N1 makes every declaration the identical zero-field message
/// — there is no content for a second one to conflict with.
#[test]
fn as_new_twice_is_not_an_error() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);

    app.run_command(&format!("override {path} --as-new mine.Foo"));
    assert!(
        !app.message.contains("already"),
        "the first declaration says nothing about reuse: {}",
        app.message
    );

    app.run_command(&format!("override {path} --as-new mine.Foo"));
    assert!(
        app.message
            .contains("mine.Foo already declared — reusing it"),
        "unexpected message: {}",
        app.message
    );
    assert!(
        app.message.contains("as mine.Foo — 1 node"),
        "the override itself still reports its blast radius: {}",
        app.message
    );
    assert_eq!(
        app.ctx.created_types(),
        ["mine.Foo".to_string()],
        "one registration, not two"
    );
}

/// Spec 0315 S4.1 and G4's other direction: the reader cannot silently
/// shadow a type the descriptor set already publishes. Refused, and the
/// refusal names the flag that does what they meant.
#[test]
fn as_new_refuses_a_real_type() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);

    app.run_command(&format!("override {path} --as-new test.Inner"));
    assert!(
        app.message.contains("'test.Inner' already exists") && app.message.contains("--as"),
        "unexpected message: {}",
        app.message
    );
    assert!(app.ctx.created_types().is_empty());
}

/// Spec 0315 S4.2/S4.3/S4.4: three names `--as-new` will not take.
///
/// The keyword refusal is the load-bearing one. `wrapper_target_for`
/// asks the pool *before* it checks its keyword rungs — deliberately,
/// so that a real message named `bool` resolves as a message — so a
/// declared `bool` would silently redefine `--as bool` for the rest of
/// the session. Asserted by using `--as bool` afterwards.
#[test]
fn as_new_refuses_a_keyword_the_internal_package_and_a_non_name() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.cursor = id_idx;
    let path = app.positional_path(id_idx);

    for (name, expected) in [
        ("bool", "is an override keyword"),
        ("none", "is an override keyword"),
        ("protolens_internal.foo", "reserved"),
        ("mine..Foo", "not a valid type name"),
        ("9lives", "not a valid type name"),
    ] {
        app.run_command(&format!("override {path} --as-new {name}"));
        assert!(
            app.message.contains(expected),
            "'{name}' must be refused with '{expected}': {}",
            app.message
        );
    }
    assert!(app.ctx.created_types().is_empty());

    app.run_command(&format!("override {path} --as bool"));
    assert_eq!(
        entry_at(&app, &OverrideOrigin::Path { path })
            .r#type
            .as_deref(),
        Some("bool"),
        "`bool` must still mean the primitive"
    );
}

/// Spec 0315 S1: the two flags are alternatives. Naming both is a parse
/// error, because there is no reading of it that is not a mistake —
/// unlike a repeated `--as`, which still means what its last one says.
#[test]
fn as_new_and_as_are_mutually_exclusive() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);
    let before = app.overrides.entries().to_vec();

    app.run_command(&format!(
        "override {path} --as test.Inner --as-new mine.Foo"
    ));
    assert!(
        app.message.contains("--as and --as-new are alternatives"),
        "unexpected message: {}",
        app.message
    );
    assert_eq!(app.overrides.entries(), before.as_slice());
    assert!(app.ctx.created_types().is_empty());

    // A repeated `--as` is not the same mistake and is not refused.
    app.run_command(&format!("override {path} --as test.Outer --as test.Inner"));
    assert_eq!(
        entry_at(&app, &OverrideOrigin::Path { path })
            .r#type
            .as_deref(),
        Some("test.Inner")
    );
}

/// Spec 0315 G2 — the one that matters. A declared type is an ordinary
/// FQDN to the origin machinery, so it anchors `fqdn:field`: naming one
/// field under it covers *every* node of that shape, which is the whole
/// reason to declare a name rather than override node by node.
///
/// Contrast is asserted too: the same sequence under `--as message`
/// falls to `path:field`, because spec 0309 refuses `fqdn:field` there —
/// all schema-free nodes share one name, so the origin would claim
/// field 1 of every node anyone ever overrode to `message`.
#[test]
fn a_declared_type_anchors_an_fqdn_field_origin() {
    let (mut app, items) = repeated_message_fixture();
    app.cursor = items[0];

    // All three `Item`s at once, so their children are three
    // structurally identical nodes under one declared type.
    app.run_command("override test.Outer:1 --as-new mine.Item");
    let first = app
        .nth_child(app.first_node, 0)
        .expect("the document still has three items");
    let child = app.first_child(first).expect("each item has one field");

    assert_eq!(
        app.override_origin_for_kind(child, Some("mine.Item"))
            .expect("the ladder always lands somewhere")
            .label(),
        "mine.Item:1",
        "spec 0308's widest-first default reaches fqdn:field under a declared anchor"
    );

    app.run_command("override mine.Item:1 --as int32");
    assert!(
        app.message.contains("mine.Item:1 as int32 — 3 nodes"),
        "one entry must cover all three occurrences: {}",
        app.message
    );

    // The contrast: `message` is refused the same rung.
    app.run_command("override test.Outer:1 --as message");
    let first = app.nth_child(app.first_node, 0).expect("still three items");
    let child = app.first_child(first).expect("each item has one field");
    assert_eq!(
        app.override_origin_for_kind(child, Some(decode::MESSAGE_KEYWORD))
            .expect("the ladder always lands somewhere")
            .kind(),
        OverrideKind::PathField,
        "spec 0309: a schema-free `message` cannot anchor fqdn:field"
    );
}

/// Spec 0315 G5/S7/S8: a declared type is visible in the two places it
/// is useful, and in neither of the two it would misrepresent.
///
/// `--as-new`'s own completion offers nothing on purpose: it would
/// offer exactly the names S4 refuses.
#[test]
fn declared_types_complete_and_list() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);
    app.run_command(&format!("override {path} --as-new mine.Foo"));

    let line = format!("override {path} --as ");
    assert_eq!(
        complete(&mut app, &format!("{line}mi")),
        format!("{line}mine.Foo")
    );

    let before = format!("override {path} --as-new mi");
    assert_eq!(
        complete(&mut app, &before),
        before,
        "--as-new completes nothing"
    );

    // `complete` leaves the command line open, and a command line takes
    // every keystroke — including the `t` that opens the pane.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // The pane opens on the lexicographic list, because that is the only
    // list the node's own declared type is in (S8 — no scored list can
    // contain it) and spec 0139 opens on the highlighted current type.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.override_sort, SortMode::Lexicographic);
    let names: Vec<&str> = app
        .override_candidates
        .iter()
        .map(|(f, _)| f.as_str())
        .collect();
    let declared = names
        .iter()
        .position(|n| *n == "mine.Foo")
        .expect("the declared type must be listed");
    let last_keyword = names
        .iter()
        .position(|n| *n == "bytes")
        .expect("the primitives are listed");
    let first_real = names
        .iter()
        .position(|n| n.starts_with("test."))
        .expect("the descriptor set's own types are listed");
    assert!(
        last_keyword < declared && declared < first_real,
        "after the keywords, before the sorted FQDNs: {names:?}"
    );
}

/// Spec 0315 G6/S10/S13: a collection using a declared type restores
/// into a fresh session with every entry intact.
///
/// The trap is `origin_resolves`, which asks the named message to
/// *declare* the field. A declared anchor has no fields by construction,
/// so without S13 every entry under one is dropped by
/// `retain_resolvable` and the restore half-applies without saying so.
#[test]
fn save_restore_round_trips_a_declared_type() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.cursor = inner_idx;
    let path = app.positional_path(inner_idx);
    app.run_command(&format!("override {path} --as-new mine.Foo"));
    app.run_command("override mine.Foo:1 --as int32 --field-name tagged");

    let file = TempFile::reserved("save-restore-declared.yaml");
    app.run_save_overrides(vec![file.as_str()]);
    let yaml = std::fs::read_to_string(file.as_str()).expect("the save must have written");
    assert!(
        yaml.contains("created_types:") && yaml.contains("mine.Foo"),
        "the declared type must be recorded: {yaml}"
    );

    // A genuinely fresh session: its own pool, which has never heard of
    // `mine.Foo`, and its own empty registry.
    let (mut fresh, _, _) = type_as_fixture();
    fresh.run_restore_overrides(vec![file.as_str()]);
    assert!(
        !fresh.message.contains("dropped"),
        "no entry may be dropped: {}",
        fresh.message
    );
    assert_eq!(fresh.ctx.created_types(), ["mine.Foo".to_string()]);
    let restored = entry_at(
        &fresh,
        &OverrideOrigin::FqdnField {
            fqdn: "mine.Foo".to_string(),
            field: 1,
        },
    );
    assert_eq!(restored.r#type.as_deref(), Some("int32"));
    assert_eq!(restored.name.as_deref(), Some("tagged"));
}
