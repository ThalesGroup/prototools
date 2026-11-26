// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Anomaly call sites (spec 0041 TC-A*, TC-B*, TC-C*).
//!
//! Most anomalies come from constructs protoc rejects at source level
//! (proto3 with extension ranges, required fields, explicit defaults): the
//! original built those descriptors with Python's protobuf; they are the
//! `#@prototext` fixtures `anomalies/`, made once by its code
//! (`protoback/tests/fixtures/gen/test_anomalies.py`). A1 and A2 compile fixtures with
//! protoc.
//!
//! Ported from `test_anomalies.py`, its command-line tests; names
//! lower-cased; its `roundtrip` marker dropped. Its unit tests, D1 and D2,
//! passed reproto's `dump_option` a Python value of the wrong type (an int
//! for a string field, a list): reproto-only, protoback's option values
//! coming typed from upb, a value of another type unrepresentable.

use crate::common::{compile_proto, encode_fixture, fixture, protoback, tmp, tools, Run};
use std::path::{Path, PathBuf};

/// The fixtures' digests, the original's bytes'.
const SETS: &[(&str, &str)] = &[
    (
        "a5_helper.pb",
        "2faebb4826d1cc9e19ed2ff13c074e8f7c81f8973bc0dfaa5a9d7e562330e2b1",
    ),
    (
        "a5_main.pb",
        "d7144b55037a8623f650c1ee82480bdebd6d4076a3e8070d533c80a180d62055",
    ),
    (
        "a5_regression.pb",
        "dcdc2d5b058869f7e154714e7b3c0ad63a751eac7264a3f1edcfd65bda3b2d09",
    ),
    (
        "b1_helper.pb",
        "3de9b38d5c32592b88be98e4262241b18fad07dd4e6d645a8623bf2f2d74f287",
    ),
    (
        "b1_main.pb",
        "3b2a8bf7a2a4509c735be45a871b8e1d16433b0c29d871963dfd063f154ad22b",
    ),
    (
        "b2.pb",
        "c62610e67fb59b538dd2433c633f96853f0d0d36326da7346b4079759a9ee559",
    ),
    (
        "b3.pb",
        "d00ac08bb4a05877f664a970045a5f08b0e9b53eb2edade85a36e5e8a4d286b2",
    ),
    (
        "c1.pb",
        "ead2756b81580d582a0302a16dafff57c0bbd9240db9452ad0bc86f0dc5d0fd7",
    ),
    (
        "c2.pb",
        "5b93e38b6d2a61396e8866ab695a47f8624d1e4683d1c364cf5eefd1180e7543",
    ),
    (
        "c3.pb",
        "937281e8552b5150b00997f3e99d8785a37b082ac9980e361dc413abe13a93c1",
    ),
    (
        "c4.pb",
        "33872d891f34bfcd0436658db8c2b83c78ff3d54ee589fba1ab75d06a31c9c60",
    ),
    (
        "weak_dep.pb",
        "401d58f0b3923521aba0d325b9adbcd79513e1b40fe3a9dd562940f240541a93",
    ),
    (
        "weak_main.pb",
        "ef49e035cb0abf3e3711b9b2f96e72932a76849d59d315d05490ba103369cc51",
    ),
];

fn run(pbs: &[PathBuf], out: &Path, extra: &[&str]) -> Run {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    protoback(&args)
}

/// The named sets encoded into `test`'s `pb`; protoback run on them, in
/// that order; the run and the output directory.
fn render(test: &str, names: &[&str]) -> (Run, PathBuf) {
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&out).unwrap();
    let pbs: Vec<PathBuf> = names
        .iter()
        .map(|n| {
            let sha = SETS.iter().find(|(f, _)| f == n).unwrap().1;
            let p = pb.join(n);
            encode_fixture(&format!("anomalies/{n}"), sha, &p);
            p
        })
        .collect();
    let r = run(&pbs, &out, &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    (r, out)
}

fn read(out: &Path, name: &str) -> String {
    std::fs::read_to_string(out.join(name)).unwrap_or_default()
}

/// --force-proto2-output on an editions file: its comment in the file, not
/// on stderr (A1, as A2, is not printed: the flag is an explicit opt-in).
#[test]
fn test_a1_editions_force_proto2_warning() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a1_editions_force_proto2_warning");
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pb_path = pb.join("editions_rendering.pb");
    let o = std::process::Command::new("protoc")
        .arg(format!("--descriptor_set_out={}", pb_path.display()))
        .arg("--include_imports")
        .arg(format!("-I{}", fixture("").display()))
        .arg(fixture("editions_rendering.proto"))
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = run(&[pb_path], &out, &["--force-proto2-output"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("editions file rendered as proto2"),
        "A1 warning must be suppressed from stderr:\n{}",
        r.stderr
    );
    assert!(read(&out, "editions_rendering.proto").contains("editions file rendered as proto2"));
}

/// --force-proto2-output on a proto3 file: no stderr warning, the
/// WARNING[downconvert] comment in the file instead.
#[test]
fn test_a2_proto3_force_proto2_warning() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_a2_proto3_force_proto2_warning");
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    // prune_base is proto3.
    let pbs = compile_proto(&pb, &["prune_base.proto"]);
    let r = run(&pbs, &out, &["--force-proto2-output"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("output syntax downconverted"),
        "A2 warning must be suppressed from stderr:\n{}",
        r.stderr
    );
    assert!(
        out.join("prune_base.proto").exists(),
        "output file must be written"
    );
    assert!(read(&out, "prune_base.proto").contains("WARNING[downconvert]"));
}

/// A proto3 file weakly importing another: A4 said, the import plain.
#[test]
fn test_a4_weak_import_proto3_warning() {
    let (r, out) = render(
        "test_a4_weak_import_proto3_warning",
        &["weak_dep.pb", "weak_main.pb"],
    );
    assert!(
        r.stderr.contains("'import weak' is not valid in proto3"),
        "{}",
        r.stderr
    );
    let content = read(&out, "weak_main.proto");
    // The warning comment holds `import weak "..."`: the import line alone.
    let import = content
        .lines()
        .find(|l| l.starts_with("import "))
        .unwrap_or("");
    assert_eq!(import, r#"import "weak_dep.proto";"#, "{content}");
}

/// A proto3 file-level extension: OMITTED[proto3], no extend block.
#[test]
fn test_a5_file_level_extend_proto3() {
    let (_, out) = render(
        "test_a5_file_level_extend_proto3",
        &["a5_helper.pb", "a5_main.pb"],
    );
    let content = read(&out, "a5_main.proto");
    assert!(content.contains("OMITTED[proto3]"), "{content}");
    assert!(
        !content.lines().any(|l| l.starts_with("extend ")),
        "{content}"
    );
}

/// Extending `.proto2.MethodOptions` in a proto3 file does not trigger A5:
/// a custom-option target, its block rendered (an orphan, its extendee in no
/// file: `///extend proto2.MethodOptions {`, spec 0459).
#[test]
fn test_a5_proto2_options_extendee_not_omitted() {
    let (_, out) = render(
        "test_a5_proto2_options_extendee_not_omitted",
        &["a5_regression.pb"],
    );
    let content = read(&out, "a5_regression.proto");
    assert!(!content.contains("OMITTED[proto3]"), "{content}");
    assert!(
        content
            .lines()
            .any(|l| l.trim_start_matches('/').starts_with("extend ")),
        "extend block must be rendered:\n{content}"
    );
}

/// A proto3 message's nested extension: OMITTED[proto3].
#[test]
fn test_b1_nested_extend_proto3() {
    let (_, out) = render(
        "test_b1_nested_extend_proto3",
        &["b1_helper.pb", "b1_main.pb"],
    );
    assert!(read(&out, "b1_main.proto").contains("OMITTED[proto3]"));
}

/// A proto3 message's extension range: OMITTED[proto3].
#[test]
fn test_b2_extension_range_proto3() {
    let (_, out) = render("test_b2_extension_range_proto3", &["b2.pb"]);
    assert!(read(&out, "b2_main.proto").contains("OMITTED[proto3]"));
}

/// A proto3 message_set_wire_format: WARNING[proto3], the option only in
/// comments.
#[test]
fn test_b3_message_set_wire_format_proto3() {
    let (_, out) = render("test_b3_message_set_wire_format_proto3", &["b3.pb"]);
    let content = read(&out, "b3_main.proto");
    assert!(content.contains("WARNING[proto3]"), "{content}");
    let syntax: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("message_set_wire_format") && !l.trim().starts_with("//"))
        .collect();
    assert!(syntax.is_empty(), "{content}");
}

/// A map entry missing its value field: C1, a repeated field in its place.
#[test]
fn test_c1_non_canonical_map_entry() {
    let (_, out) = render("test_c1_non_canonical_map_entry", &["c1.pb"]);
    let content = read(&out, "c1_main.proto");
    assert!(content.contains("WARNING[render]"), "{content}");
    assert!(
        content.contains("repeated"),
        "Fallback repeated field must be rendered"
    );
}

/// A proto3 group field: WARNING[proto3].
#[test]
fn test_c2_group_field_proto3() {
    let (_, out) = render("test_c2_group_field_proto3", &["c2.pb"]);
    assert!(read(&out, "c2_main.proto").contains("WARNING[proto3]"));
}

/// A proto3 required field: WARNING[proto3], no `required` on the line.
#[test]
fn test_c3_required_field_proto3() {
    let (_, out) = render("test_c3_required_field_proto3", &["c3.pb"]);
    let content = read(&out, "c3_main.proto");
    assert!(content.contains("WARNING[proto3]"), "{content}");
    let lines: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("mandatory") && !l.trim().starts_with("//"))
        .collect();
    assert!(
        !lines.is_empty() && lines.iter().all(|l| !l.contains("required")),
        "{content}"
    );
}

/// A proto3 field with a default value: WARNING[proto3].
#[test]
fn test_c4_explicit_default_proto3() {
    let (_, out) = render("test_c4_explicit_default_proto3", &["c4.pb"]);
    assert!(read(&out, "c4_main.proto").contains("WARNING[proto3]"));
}
