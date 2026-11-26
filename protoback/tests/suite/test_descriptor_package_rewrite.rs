// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The renamed descriptor's package, consistent (specs 0159, 0447 S3).
//!
//! Regression tests for a bug where a renaming rewrote a file's own type
//! references (type_name/extendee/input_type/output_type) without rewriting
//! that same file's own `package` field, producing an internally
//! self-inconsistent FileDescriptorProto that fails to load into a real
//! DescriptorPool whenever the file contains a same-package self-reference:
//! most severely the descriptor's own file, where every field is one. Under
//! --target protoc the descriptor's definitions are renamed to
//! google.protobuf, its package with them, in its file alone.
//!
//! Ported from `test_descriptor_package_rewrite.py`. The pool its set must
//! load into is protoc's (`--descriptor_set_in`), where the original used
//! Python's `DescriptorPool`; the type it looked up there is read from the
//! set. Its tests' names are the original's, lower-cased (`test_G1_…` is
//! `test_g1_…`), as Rust names functions.

use crate::common::{protoback, protoc_loads, set_files, tmp, tools, vendor_descriptor, Run};
use std::path::Path;

const DESCRIPTOR: &str = "vendor/proto/descriptor.proto";
/// Definitions of the descriptor's file, beside its options messages.
const SELF_REFERENCES: &str = "
message Outer { optional Inner inner = 1; }
message Inner { optional string value = 1; }
message ItemSet { option message_set_wire_format = true; extensions 4 to max; }
";

/// The descriptor vendored in `dir`, rendered with `flags`; its schema DB
/// `dir/schema.desc` (`_setup_fixtures` and `_run`, which
/// `test_scoring_graph_namespace_consistency` uses too).
pub fn run(dir: &Path, flags: &[&str]) -> Run {
    vendor_descriptor(dir, "", SELF_REFERENCES);
    // Named: nothing here extends its options, for spec 0447 S1 to infer it.
    let mut args: Vec<std::ffi::OsString> = vec![
        "--descriptor".into(),
        DESCRIPTOR.into(),
        format!("-I{}", dir.join("in").display()).into(),
        format!("--proto-out={}", dir.join("out").display()).into(),
        format!("--schema-db-out={}", dir.join("schema.desc").display()).into(),
    ];
    args.extend(flags.iter().map(Into::into));
    protoback(args)
}

fn inner_type(dir: &Path, file: &str) -> String {
    let files = set_files(&dir.join("schema.desc"));
    let f = files.iter().find(|f| f.name == file).unwrap();
    let outer = f.messages.iter().find(|(m, _)| m == "Outer").unwrap();
    outer
        .1
        .iter()
        .find(|(n, _)| n == "inner")
        .unwrap()
        .1
        .clone()
}

#[test]
fn test_g1_protoc_self_reference_loads_into_pool() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_g1_protoc_self_reference_loads_into_pool");
    let result = run(&dir, &["--target", "protoc"]);
    assert_eq!(result.code, Some(0), "protoback failed:\n{}", result.stderr);
    protoc_loads(
        &dir.join("schema.desc"),
        &["google/protobuf/descriptor.proto"],
    );
    assert_eq!(
        inner_type(&dir, "google/protobuf/descriptor.proto"),
        ".google.protobuf.Inner"
    );
}

#[test]
fn test_g2_keep_stays_fully_unrewritten() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_g2_keep_stays_fully_unrewritten");
    let result = run(&dir, &[]);
    assert_eq!(result.code, Some(0), "protoback failed:\n{}", result.stderr);
    let files = set_files(&dir.join("schema.desc"));
    let desc = files.iter().find(|f| f.name == DESCRIPTOR).unwrap();
    assert_eq!(desc.package, "vendor");
    assert_eq!(inner_type(&dir, DESCRIPTOR), ".vendor.Inner");
    protoc_loads(&dir.join("schema.desc"), &[DESCRIPTOR]);
}
