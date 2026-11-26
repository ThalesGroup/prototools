// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! hopcroft.rkyv / .desc name consistency (spec 0166).
//!
//! Regression tests for a bug where a renaming was applied to .desc's names
//! at render time (spec 0159) but not to hopcroft.rkyv's, read off the pool
//! before any renaming exists: a consumer scoring by .desc's renamed name
//! got "not found in scoring graph" for types present under their old name.
//! The renaming is --target protoc's (spec 0447 S3): the vendor descriptor's
//! definitions renamed to google.protobuf.
//!
//! hopcroft.rkyv's content needs a real reader: `prototext score --type
//! <NAME>`, which fails with "type '<NAME>' not found in scoring graph" if
//! the name is absent, the failure first reported (spec 0488 D4).
//!
//! Ported from `test_scoring_graph_namespace_consistency.py`, on
//! `test_descriptor_package_rewrite`'s setup as it was; its tests' names
//! lower-cased.

use crate::common::{prototext, tmp, tools, Output};
use crate::test_descriptor_package_rewrite::run;
use std::path::Path;

const OUTER_PAYLOAD: &[u8] = b"\x0a\x03\x0a\x01x";

/// A MessageSet's synthesized `Item` group (spec 0108): group(1) {
/// type_id: int32(2) = 1000, message: bytes(3) = "" }, the whole payload of
/// a message_set_wire_format message.
const ITEM_PAYLOAD: &[u8] = &[0x0B, 0x10, 0xE8, 0x07, 0x1A, 0x00, 0x0C];

fn score(dir: &Path, type_name: &str, payload: &[u8]) -> Output {
    let path = dir.join(format!("{}.bin", type_name.replace('.', "_")));
    std::fs::write(&path, payload).unwrap();
    prototext([
        "--descriptor-set".into(),
        dir.join("schema.desc").into_os_string(),
        "score".into(),
        "--type".into(),
        type_name.into(),
        "--assume-binary".into(),
        path.into_os_string(),
    ])
}

/// hopcroft.rkyv's names track .desc's renamed ones (spec 0166 G1).
#[test]
fn test_g1_renamed_name_is_a_scoring_graph_root_unrenamed_is_not() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_g1_renamed_name_is_a_scoring_graph_root_unrenamed_is_not");
    let r = run(&dir, &["--target", "protoc"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let renamed = score(&dir, "google.protobuf.Outer", OUTER_PAYLOAD);
    assert_eq!(
        renamed.code,
        Some(0),
        "scoring google.protobuf.Outer failed:\n{}",
        renamed.stderr
    );
    let original = score(&dir, "vendor.Outer", OUTER_PAYLOAD);
    assert_ne!(
        original.code,
        Some(0),
        "vendor.Outer must not be a scoring-graph root"
    );
    assert!(
        original.stderr.contains("not found in scoring graph"),
        "{}",
        original.stderr
    );
}

/// --target keep: hopcroft.rkyv stays in the original namespace too,
/// matching .desc (spec 0166 G3), the inverse of G1.
#[test]
fn test_g3_keep_scoring_graph_stays_unrenamed() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_g3_keep_scoring_graph_stays_unrenamed");
    let r = run(&dir, &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let original = score(&dir, "vendor.Outer", OUTER_PAYLOAD);
    assert_eq!(
        original.code,
        Some(0),
        "scoring vendor.Outer failed:\n{}",
        original.stderr
    );
    let renamed = score(&dir, "google.protobuf.Outer", OUTER_PAYLOAD);
    assert_ne!(
        renamed.code,
        Some(0),
        "google.protobuf.Outer must not be a scoring-graph root"
    );
    assert!(
        renamed.stderr.contains("not found in scoring graph"),
        "{}",
        renamed.stderr
    );
}

/// A MessageSet message's synthesized `Item` (spec 0108) is renamed with its
/// parent (spec 0166 S3): its payload scores under the renamed name, with
/// real match credit.
#[test]
fn test_regression_message_set_item_lands_in_renamed_namespace() {
    if !tools(&["protoc", "prototext"]) {
        return;
    }
    let dir = tmp("test_regression_message_set_item_lands_in_renamed_namespace");
    let r = run(&dir, &["--target", "protoc"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let renamed = score(&dir, "google.protobuf.ItemSet", ITEM_PAYLOAD);
    assert_eq!(
        renamed.code,
        Some(0),
        "scoring google.protobuf.ItemSet failed:\n{}",
        renamed.stderr
    );
    assert!(
        renamed.stdout.contains("matches: 3"),
        "expected the synthesized Item group/type_id/message to all register as matches:\n{}",
        renamed.stdout
    );
}
