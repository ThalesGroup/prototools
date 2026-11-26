// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0447 S3, S6, S7: --target, --editions, and profiles.
//!
//! A profile is a saved set of options, found as a path, in
//! $PROTOBACK_PROFILE_PATH, or among the built-in ones; the command line
//! overrides it, lists accumulate. Its folder's files replace the input's,
//! with W29 when the replacement lacks a definition of the copy it
//! replaces.
//!
//! Ported from `test_profiles.py`.

use crate::common::{protoback_command, tmp, tools, vendor_descriptor, write, Run};
use std::path::{Path, PathBuf};

const EDITIONS: &str = r#"name: "e.proto" package: "e" syntax: "editions" edition: EDITION_2023
message_type { name: "E" field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }
"#;
const TWO_FIELDS: &str = r#"name: "a.proto" package: "a" message_type { name: "A"
  field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 }
  field { name: "y" number: 2 label: LABEL_OPTIONAL type: TYPE_INT32 } }
"#;
const ONE_FIELD: &str = r#"name: "a.proto" package: "a" message_type { name: "A"
  field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }
"#;

/// `files` written in `dir/in`, rendered into `dir/out` with `flags`, from
/// `dir`, `PROTOBACK_PROFILE_PATH` set to `profile_path` or unset.
fn run(dir: &Path, files: &[(&str, &str)], flags: &[&str], profile_path: Option<&Path>) -> Run {
    let inp = dir.join("in");
    for (name, text) in files {
        write(&inp.join(name), text);
    }
    let mut args: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
    args.extend([
        format!("-I{}", inp.display()),
        "-O".into(),
        dir.join("out").display().to_string(),
    ]);
    let mut c = protoback_command(&args);
    c.current_dir(dir).env_remove("PROTOBACK_PROFILE_PATH");
    if let Some(p) = profile_path {
        c.env("PROTOBACK_PROFILE_PATH", p);
    }
    let o = c.output().unwrap();
    Run {
        code: o.status.code(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join("out").join(name)).unwrap()
}

/// A profiles directory under `dir`, `profs`.
fn profs(dir: &Path) -> PathBuf {
    let p = dir.join("profs");
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn test_the_builtin_prost_profile_writes_editions_as_proto2() {
    let dir = tmp("test_the_builtin_prost_profile_writes_editions_as_proto2");
    let r = run(
        &dir,
        &[("e.textpb", EDITIONS)],
        &["--profile", "prost"],
        None,
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let e = read(&dir, "e.proto");
    assert!(e.contains(r#"syntax = "proto2";"#), "{e}");
}

#[test]
fn test_the_command_line_overrides_a_profile() {
    let dir = tmp("test_the_command_line_overrides_a_profile");
    let r = run(
        &dir,
        &[("e.textpb", EDITIONS)],
        &["--profile", "prost", "--editions", "keep"],
        None,
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let e = read(&dir, "e.proto");
    assert!(e.contains(r#"edition = "2023";"#), "{e}");
}

#[test]
fn test_a_profile_is_found_as_a_path() {
    let dir = tmp("test_a_profile_is_found_as_a_path");
    write(&dir.join("mine.yaml"), "editions: proto2\n");
    let r = run(
        &dir,
        &[("e.textpb", EDITIONS)],
        &["--profile", "mine"],
        None,
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let e = read(&dir, "e.proto");
    assert!(e.contains(r#"syntax = "proto2";"#), "{e}");
}

#[test]
fn test_a_profile_not_found_is_a_usage_error() {
    let dir = tmp("test_a_profile_not_found_is_a_usage_error");
    let r = run(
        &dir,
        &[("a.textpb", ONE_FIELD)],
        &["--profile", "ghost"],
        None,
    );
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(
        r.stderr.contains(
            "profile 'ghost' not found (looked in: the current directory, the built-in profiles)"
        ),
        "{}",
        r.stderr
    );
}

#[test]
fn test_an_unknown_key_is_a_usage_error() {
    let dir = tmp("test_an_unknown_key_is_a_usage_error");
    let profs = profs(&dir);
    write(&profs.join("bad.yaml"), "target: keep\nnope: 1\n");
    let r = run(
        &dir,
        &[("a.textpb", ONE_FIELD)],
        &["--profile", "bad"],
        Some(&profs),
    );
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(
        r.stderr.contains("profile 'bad': unknown key(s): nope"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_a_profile_folder_replaces_the_input_and_w29_says_what_is_lost() {
    let dir = tmp("test_a_profile_folder_replaces_the_input_and_w29_says_what_is_lost");
    let profs = profs(&dir);
    write(&profs.join("rich.yaml"), "fail_on: [W29]\n");
    write(&profs.join("rich/a.textpb"), ONE_FIELD);
    let r = run(
        &dir,
        &[("a.textpb", TWO_FIELDS)],
        &["--profile", "rich"],
        Some(&profs),
    );
    let said = format!(
        "Warning [W29]: 'a.proto': the copy in {} lacks what the input's defines: a.A.y",
        profs.join("rich").display()
    );
    assert!(r.stderr.contains(&said), "{}", r.stderr);
    // fail_on came from the profile.
    assert_eq!(r.code, Some(3), "{}", r.stderr);
    let a = read(&dir, "a.proto");
    assert!(a.contains("int32 x = 1") && !a.contains("int32 y"), "{a}");
}

#[test]
fn test_a_superset_replaces_silently() {
    let dir = tmp("test_a_superset_replaces_silently");
    let profs = profs(&dir);
    write(&profs.join("rich.yaml"), "fail_on: [W29]\n");
    write(&profs.join("rich/a.textpb"), TWO_FIELDS);
    let r = run(
        &dir,
        &[("a.textpb", ONE_FIELD)],
        &["--profile", "rich"],
        Some(&profs),
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let a = read(&dir, "a.proto");
    assert!(a.contains("int32 y = 2"), "{a}");
}

const CUSTOM: &str = r#"name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
"#;

/// Spec 0447 test plan 5: `target: protoc` in a profile is --target protoc.
#[test]
fn test_target_key_in_a_profile() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_target_key_in_a_profile");
    vendor_descriptor(&dir, "", "");
    write(&dir.join("vendor-protoc.yaml"), "target: protoc\n");
    let r = run(
        &dir,
        &[("c.textpb", CUSTOM)],
        &["--profile", "vendor-protoc"],
        None,
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let c = read(&dir, "c.proto");
    assert!(c.contains("extend google.protobuf.FieldOptions {"), "{c}");
    assert!(dir.join("out/google/protobuf/descriptor.proto").exists());
}

/// Spec 0447 test plan 5: a later profile's scalar wins; lists accumulate.
/// The first, with a folder replacing a.proto by a copy lacking a field.
#[test]
fn test_profiles_compose() {
    let dir = tmp("test_profiles_compose");
    let profs = profs(&dir);
    write(
        &profs.join("first.yaml"),
        "editions: proto2\nfail_on: [W29]\n",
    );
    write(&profs.join("first/a.textpb"), ONE_FIELD);
    write(&dir.join("second.yaml"), "editions: keep\nfail_on: [W1]\n");
    let r = run(
        &dir,
        &[("e.textpb", EDITIONS), ("a.textpb", TWO_FIELDS)],
        &["--profile", "profs/first", "--profile", "second"],
        None,
    );
    let e = read(&dir, "e.proto");
    assert!(e.contains(r#"edition = "2023";"#), "{e}");
    // fail_on accumulated: the first profile's W29 still fails the run.
    assert!(r.stderr.contains("Warning [W29]"), "{}", r.stderr);
    assert_eq!(r.code, Some(3), "{}", r.stderr);
}
