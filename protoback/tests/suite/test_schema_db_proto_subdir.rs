// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Tests for -O/--schema-db-out stub-directory overlap validation (spec 0155
//! G1).
//!
//! The validation runs before any DESCRIPTOR_FILES are loaded, so these
//! tests pass a nonexistent dummy file as the positional argument and only
//! assert on the specific usage error's text (or its absence).
//!
//! Ported from `test_schema_db_proto_subdir.py`, whose `PYTHONPATH` setup
//! was reproto's own; its tests' names lower-cased (`test_G1_` is
//! `test_g1_`), as Rust names functions.

use crate::common::{protoback, tmp, Run};
use std::path::Path;

const ERROR_MARKER: &str = "only an immediate 'proto' child directory is allowed";

fn run(db: &Path, proto_out: &Path, dummy: &Path) -> Run {
    protoback([
        format!("--schema-db-out={}", db.display()),
        format!("-O{}", proto_out.display()),
        dummy.display().to_string(),
    ])
}

#[test]
fn test_g1_proto_child_of_stub_is_allowed() {
    let dir = tmp("test_g1_proto_child_of_stub_is_allowed");
    let r = run(
        &dir.join("schema.desc"),
        &dir.join("schema/proto"),
        &dir.join("missing.pb"),
    );
    assert!(!r.stderr.contains(ERROR_MARKER));
}

#[test]
fn test_g1_stub_itself_is_forbidden() {
    let dir = tmp("test_g1_stub_itself_is_forbidden");
    let r = run(
        &dir.join("schema.desc"),
        &dir.join("schema"),
        &dir.join("missing.pb"),
    );
    assert_ne!(r.code, Some(0));
    assert!(r.stderr.contains(ERROR_MARKER), "{}", r.stderr);
}

#[test]
fn test_g1_differently_named_child_is_forbidden() {
    let dir = tmp("test_g1_differently_named_child_is_forbidden");
    let r = run(
        &dir.join("schema.desc"),
        &dir.join("schema/desc"),
        &dir.join("missing.pb"),
    );
    assert_ne!(r.code, Some(0));
    assert!(r.stderr.contains(ERROR_MARKER), "{}", r.stderr);
}

#[test]
fn test_g1_nested_proto_is_forbidden() {
    let dir = tmp("test_g1_nested_proto_is_forbidden");
    let r = run(
        &dir.join("schema.desc"),
        &dir.join("schema/nested/proto"),
        &dir.join("missing.pb"),
    );
    assert_ne!(r.code, Some(0));
    assert!(r.stderr.contains(ERROR_MARKER), "{}", r.stderr);
}

#[test]
fn test_g1_outside_stub_is_allowed() {
    let dir = tmp("test_g1_outside_stub_is_allowed");
    let r = run(
        &dir.join("schema.desc"),
        &dir.join("elsewhere"),
        &dir.join("missing.pb"),
    );
    assert!(!r.stderr.contains(ERROR_MARKER));
}

#[test]
fn test_n1_no_schema_db_out_is_unaffected() {
    let dir = tmp("test_n1_no_schema_db_out_is_unaffected");
    let r = protoback([
        format!("-O{}", dir.join("out").display()),
        dir.join("missing.pb").display().to_string(),
    ]);
    assert!(!r.stderr.contains(ERROR_MARKER));
}
