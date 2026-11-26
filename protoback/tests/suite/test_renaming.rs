// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The renaming --target protoc derives, written (specs 0440, 0447 S3).
//!
//! The vendor descriptor's definitions renamed to `google.protobuf`: names
//! are written as spec 0434 writes them, in the output namespace (0440 S3),
//! in the importers and in the descriptor itself, whose package is the stock
//! one in its file alone; two symbols renamed onto one name stop the run
//! (0440 S2).
//!
//! Ported from `test_renaming.py`.

use crate::common::{protoback, tmp, tools, vendor_descriptor, write, Run};
use std::path::Path;

const USE: &str = r#"name: "app/use.proto" package: "app" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50201 label: LABEL_OPTIONAL type: TYPE_STRING
  extendee: ".vendor.FieldOptions" }
message_type { name: "M"
  field { name: "mode" number: 1 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" options { 50201: "t" } } }
message_type { name: "N"
  nested_type { name: "google" }
  field { name: "mode" number: 1 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" } }
"#;

/// A file of protoc's own package, beside the vendor descriptor.
const EMPTY: &str = r#"name: "google/protobuf/empty.proto" package: "google.protobuf"
message_type { name: "Empty" }
"#;

fn run(dir: &Path, top: &str, files: &[(&str, &str)]) -> Run {
    vendor_descriptor(dir, "", top);
    let inp = dir.join("in");
    for (name, text) in files {
        write(&inp.join(name), text);
    }
    protoback([
        "--target".as_ref(),
        "protoc".as_ref(),
        format!("-I{}", inp.display()).as_ref(),
        "-O".as_ref(),
        dir.join("out").as_os_str(),
    ])
}

#[test]
fn test_renamed_names_are_written_in_the_output_namespace() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_renamed_names_are_written_in_the_output_namespace");
    let result = run(&dir, "", &[("use.textpb", USE)]);
    assert_eq!(result.code, Some(0), "{}", result.stderr);
    let out = dir.join("out");
    let use_ = std::fs::read_to_string(out.join("app/use.proto")).unwrap();
    // From another package: in full, no dot.
    assert!(
        use_.contains("optional google.protobuf.FileOptions.OptimizeMode mode = 1"),
        "{use_}"
    );
    // Where `google` is captured (N.google): the dot.
    assert!(
        use_.contains("optional .google.protobuf.FileOptions.OptimizeMode mode = 1;"),
        "{use_}"
    );
    assert!(
        use_.contains("extend google.protobuf.FieldOptions {"),
        "{use_}"
    );
    // Within the descriptor, its own output package: short.
    let desc = std::fs::read_to_string(out.join("google/protobuf/descriptor.proto")).unwrap();
    assert!(
        desc.contains("optional OptimizeMode optimize_for = 9"),
        "{desc}"
    );
    assert!(
        desc.contains("optional FeatureSet features = 50;"),
        "{desc}"
    );
    let compiled = std::process::Command::new("protoc")
        .arg(format!("-I{}", out.display()))
        .args(["-o", "/dev/null", "app/use.proto"])
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

#[test]
fn test_two_symbols_renamed_onto_one_name_stop_the_run() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_two_symbols_renamed_onto_one_name_stop_the_run");
    let result = run(
        &dir,
        "message Empty { optional int32 marker = 1; }\n",
        &[("use.textpb", USE), ("empty.textpb", EMPTY)],
    );
    assert_eq!(result.code, Some(1), "{}", result.stderr);
    assert!(
        result.stderr.contains(
            "'google/protobuf/empty.proto' and 'vendor/proto/descriptor.proto' both define \
             'google.protobuf.Empty' once rewritten; prune one (-p)"
        ),
        "{}",
        result.stderr
    );
    // The outermost name only: not its field.
    assert!(!result.stderr.contains("Empty.marker"));
    assert!(!dir.join("out/app/use.proto").exists());
}
