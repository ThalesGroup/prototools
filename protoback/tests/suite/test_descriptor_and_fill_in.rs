// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0447 S1 and S2: the descriptor the input was compiled against,
//! inferred; built-in copies filling in what the input lacks.
//!
//! A stock input needs no option: a missing descriptor.proto or well-known
//! file is filled in, silently. A vendor input's descriptor is inferred from
//! what its custom options extend; extendees of two vendors are an error,
//! which --descriptor settles.
//!
//! Ported from `test_descriptor_and_fill_in.py`. The `needs_release_protobuf`
//! marker of its first test is dropped: a constraint of the Python protobuf
//! beside reproto, which protoback, embedding upb at the release, never meets.

use crate::common::{protoback, tmp, tools, vendor_descriptor, write, Run};
use std::path::Path;

const USER: &str = r#"name: "b.proto" package: "b" dependency: "google/protobuf/timestamp.proto"
message_type { name: "B" field { name: "t" number: 1 label: LABEL_OPTIONAL
  type: TYPE_MESSAGE type_name: ".google.protobuf.Timestamp" } }
"#;
const CUSTOM: &str = r#"name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
message_type { name: "M" field { name: "f" number: 1 label: LABEL_OPTIONAL
  type: TYPE_INT32 options { 50000: 7 } } }
"#;
const OTHER: &str = r#"name: "d.proto" package: "d" extension { name: "x" number: 50001
  label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".other.FileOptions" }
"#;

fn run(dir: &Path, files: &[(&str, &str)], flags: &[&str]) -> Run {
    let inp = dir.join("in");
    std::fs::create_dir_all(&inp).unwrap();
    for (name, text) in files {
        write(&inp.join(name), text);
    }
    let mut args: Vec<std::ffi::OsString> = flags.iter().map(Into::into).collect();
    args.push(format!("-I{}", inp.display()).into());
    args.extend(["-O".into(), dir.join("out").into_os_string()]);
    protoback(args)
}

#[test]
fn test_a_stock_input_needs_no_option() {
    let dir = tmp("test_a_stock_input_needs_no_option");
    let r = run(&dir, &[("b.textpb", USER)], &[]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(
        !r.stderr.contains("Warning") && !r.stderr.contains("fallback"),
        "{}",
        r.stderr
    );
    let b = std::fs::read_to_string(dir.join("out/b.proto")).unwrap();
    assert!(b.contains("google.protobuf.Timestamp t = 1"), "{b}");
    assert!(dir.join("out/google/protobuf/timestamp.proto").exists());
}

#[test]
fn test_no_builtin_fills_in_nothing() {
    let dir = tmp("test_no_builtin_fills_in_nothing");
    let r = run(&dir, &[("b.textpb", USER)], &["--no-builtin"]);
    assert_eq!(r.code, Some(1), "{}", r.stderr);
    assert!(
        r.stderr
            .contains("descriptor.proto not found in the input set"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_use_variant_is_deprecated() {
    let dir = tmp("test_use_variant_is_deprecated");
    let r = run(
        &dir,
        &[("b.textpb", USER)],
        &["--use-variant", "descriptor"],
    );
    assert!(
        r.stderr
            .contains("Warning [W18]: --use-variant is deprecated; use --builtin"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_a_vendor_descriptor_is_inferred() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a_vendor_descriptor_is_inferred");
    vendor_descriptor(&dir, "", "");
    let r = run(&dir, &[("c.textpb", CUSTOM)], &[]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let c = std::fs::read_to_string(dir.join("out/c.proto")).unwrap();
    assert!(c.contains("[(c.tag) = 7]"), "{c}");
    assert!(!r.stderr.contains("W22"), "{}", r.stderr);
}

#[test]
fn test_descriptor_overrides_the_inference() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_descriptor_overrides_the_inference");
    vendor_descriptor(&dir, "", "");
    let r = run(
        &dir,
        &[("c.textpb", CUSTOM)],
        &["--descriptor", "google/protobuf/descriptor.proto"],
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    // Read against protoc's descriptor, the vendor's option is an orphan.
    assert!(r.stderr.contains("W22"), "{}", r.stderr);
}

#[test]
fn test_two_vendors_are_an_error() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_two_vendors_are_an_error");
    vendor_descriptor(&dir, "", "");
    let r = run(&dir, &[("c.textpb", CUSTOM), ("d.textpb", OTHER)], &[]);
    assert_eq!(r.code, Some(1), "{}", r.stderr);
    assert!(
        r.stderr.contains(
            "extend the options of several descriptors: other (as in d.proto), \
             vendor (as in c.proto); name the one to use with --descriptor"
        ),
        "{}",
        r.stderr
    );
}
