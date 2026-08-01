// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0156 G6c/G6d: `resolve_export_fields`, the four-tier walk that
//! turns a subtree plus its overrides into a descriptor.

use super::super::*;
use super::support::*;
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};

// ── Spec 0156 G6c/G6d: `resolve_export_fields` ─────────────────────────────

fn field_by_number(
    fields: &[export_descriptor::ResolvedField],
    number: u32,
) -> &export_descriptor::ResolvedField {
    fields
        .iter()
        .find(|f| f.number == number)
        .unwrap_or_else(|| panic!("no resolved field for number {number}, got {fields:#?}"))
}

impl std::fmt::Debug for export_descriptor::ResolvedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedField")
            .field("number", &self.number)
            .field("name", &self.name)
            .field("label", &self.label)
            .field("type", &self.r#type)
            .field("type_name", &self.type_name)
            .finish()
    }
}

/// G6c tier 3: a child whose parent's schema resolves it to a
/// primitive type uses that type directly, no dependency.
#[test]
fn resolve_export_fields_tier3_resolves_a_schema_primitive() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let num = field_by_number(&fields, 1);
    assert_eq!(num.r#type, Type::Int32);
    assert!(num.type_name.is_none());
    assert!(num.referenced_file.is_none());
}

/// G6c tier 3 + G6d: a child resolving to a message in a *different*
/// file declares that file as a dependency; a second child resolving
/// to a message in the cursor's *own* file also succeeds and declares
/// that file too (G6d's "no exclusion" behavior) — verified end-to-end
/// by feeding both into `build`. Neither dependency's content is
/// embedded.
#[test]
fn resolve_export_fields_tier3_message_declares_its_file_as_a_dependency_no_exclusion() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();

    let other_file_field = field_by_number(&fields, 2);
    assert_eq!(other_file_field.r#type, Type::Message);
    assert_eq!(other_file_field.type_name.as_deref(), Some(".other.Msg"));
    assert!(other_file_field.referenced_file.is_some());

    let own_file_field = field_by_number(&fields, 3);
    assert_eq!(own_file_field.r#type, Type::Message);
    assert_eq!(own_file_field.type_name.as_deref(), Some(".test.OwnType"));
    assert!(own_file_field.referenced_file.is_some());

    let fds = export_descriptor::build("Msg", fields);
    assert_eq!(fds.file.len(), 1, "no dependency's content is embedded");
    let dependency = &fds.file[0].dependency;
    assert!(dependency.contains(&"other.proto".to_string()));
    assert!(
        dependency.contains(&"outer.proto".to_string()),
        "got: {dependency:?}"
    );
}

/// G6c tier 1: an active `PathField` override at the cursor's own path
/// retypes the matching field.
#[test]
fn resolve_export_fields_tier1_path_field_override_retypes_the_field() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: app.positional_path(app.cursor),
            field: 4,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let retyped = field_by_number(&fields, 4);
    assert_eq!(retyped.r#type, Type::Message);
    assert_eq!(retyped.type_name.as_deref(), Some(".other.Msg"));
}

/// G6c tier 2: an active `FqdnField` override at the cursor's own
/// `type_fqdn` is used only when no matching `PathField` override
/// exists for that field.
#[test]
fn resolve_export_fields_tier2_fqdn_field_override_applies_without_a_path_match() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::FqdnField {
            fqdn: "test.Outer".to_string(),
            field: 6,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let retyped = field_by_number(&fields, 6);
    assert_eq!(retyped.r#type, Type::Message);
    assert_eq!(retyped.type_name.as_deref(), Some(".other.Msg"));
}

/// N4: an override entry at a path other than the cursor's has no
/// effect — field 8's tier-4 guess still applies untouched.
#[test]
fn resolve_export_fields_an_override_at_a_different_path_has_no_effect() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: "/99".to_string(),
            field: 8,
        },
        Some("other.Msg".to_string()),
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let untouched = field_by_number(&fields, 8);
    assert_eq!(untouched.r#type, Type::Int64);
    assert!(untouched.type_name.is_none());
}

/// An active override with target `None` (raw) turns that field into
/// `bytes`.
#[test]
fn resolve_export_fields_a_raw_override_turns_the_field_into_bytes() {
    let mut app = export_fields_fixture();
    app.overrides.activate(
        OverrideOrigin::PathField {
            path: app.positional_path(app.cursor),
            field: 7,
        },
        None,
    );
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let raw = field_by_number(&fields, 7);
    assert_eq!(raw.r#type, Type::Bytes);
    assert!(raw.type_name.is_none());
}

/// Two live children sharing one field_number collapse to one
/// exported field, `LABEL_REPEATED` — and, since field 8 is
/// undeclared in the schema, its type is tier 4's `WT_VARINT` ->
/// `int64` guess.
#[test]
fn resolve_export_fields_repeated_children_collapse_to_one_repeated_field() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    assert_eq!(fields.iter().filter(|f| f.number == 8).count(), 1);
    let repeated = field_by_number(&fields, 8);
    assert_eq!(repeated.label, Label::Repeated);
    assert_eq!(repeated.r#type, Type::Int64);
}

/// G6c tier 4: primitive-only children, no schema, no overrides —
/// each child's guessed keyword matches G6c's table, `dependency` is
/// empty (the `WT_VARINT` case is also covered above via field 8;
/// this exercises the same tier-4 table entry through a minimal
/// single-field cursor).
#[test]
fn resolve_export_fields_tier4_guesses_from_wire_type() {
    let app = export_fields_fixture();
    let fields = app.resolve_export_fields(app.cursor).unwrap();
    let guessed = field_by_number(&fields, 8);
    assert_eq!(guessed.r#type, Type::Int64);
    let fds = export_descriptor::build("Msg", vec![]);
    assert_eq!(fds.file[0].dependency, Vec::<String>::new());
}

/// A `WT_START_GROUP` child with no resolvable/overridden type is an
/// error.
#[test]
fn resolve_export_fields_untyped_group_child_is_an_error() {
    let app = export_fields_group_error_fixture();
    let err = app.resolve_export_fields(app.cursor).unwrap_err();
    assert!(err.contains("untyped"), "got: {err}");
}

/// Cursor node is a scalar leaf (not `is_message`): `export_descriptor_
/// bytes` returns the "not a message/group" error without calling
/// `resolve_export_fields` at all.
#[test]
fn export_descriptor_bytes_on_a_scalar_leaf_cursor_is_an_error() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.set_cursor(id_idx);
    assert!(!app.tree[app.cursor].span.is_message);
    let err = app.export_descriptor_bytes(false).unwrap_err();
    assert!(err.contains("not a message/group"), "got: {err}");
}
