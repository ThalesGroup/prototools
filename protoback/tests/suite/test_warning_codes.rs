// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Warning codes, counted, and made fatal on request (spec 0438).
//!
//! Ported from `test_warning_codes.py`, its one-parameter-list test running
//! its parameters in turn. Its inputs, built with Python's protobuf, are the
//! fixtures `warning_codes/`, made once by its code
//! (`protoback/tests/fixtures/gen/test_warning_codes.py`): `a.textpb` as protoc printed
//! and the original edited it, `b.pb` (`#@prototext`) with its undeclared
//! group.

use crate::common::{encode_fixture, fixture, protoback, tmp, Run};
use std::path::{Path, PathBuf};

/// The `inputs` fixture: a.proto importing b.proto, which the input lacks
/// (W5); its options as text, one by a name nothing declares (T4) and one by
/// a number nothing declares (W22 once rendered).
fn inputs(test: &str) -> PathBuf {
    let d = tmp(test).join("in");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(fixture("warning_codes/a.textpb"), d.join("a.textpb")).unwrap();
    d
}

fn run(inputs: &Path, args: &[&str]) -> Run {
    let out = inputs.parent().unwrap().join("out");
    let mut all: Vec<String> = vec![
        format!("-I{}", inputs.display()),
        "-O".into(),
        out.display().to_string(),
        "--use-variant".into(),
        "descriptor".into(),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    all.push("a.textpb".into());
    protoback(&all)
}

#[test]
fn test_every_warning_line_shows_its_code_and_text_ones_are_counted() {
    let r = run(
        &inputs("test_every_warning_line_shows_its_code_and_text_ones_are_counted"),
        &[],
    );
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let lines: Vec<&str> = r.stderr.lines().collect();
    // Missing while loading (W1) and while rendering (W5): one summary line.
    for s in [
        "Warning [W5]: missing dependency file:b.proto (2 occurrences)",
        "Warning [T4]: field no_such_option of google.protobuf.FileOptions is declared nowhere in the input: skipped (1 occurrence)",
        "Warning [W22]: 'a.proto': option field 99 of google.protobuf.FileOptions is declared nowhere: written as an orphan (1 occurrence)",
    ] {
        assert!(lines.contains(&s), "{s}:\n{}", r.stderr);
    }
    assert!(
        !lines.iter().any(|l| l.starts_with("Warning:")),
        "{}",
        r.stderr
    );
}

#[test]
fn test_an_undeclared_option_field_is_an_orphan_redacted_or_not() {
    let inp = inputs("test_an_undeclared_option_field_is_an_orphan_redacted_or_not");
    let out = inp.parent().unwrap().join("out/a.proto");
    assert_eq!(run(&inp, &[]).code, Some(0));
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("///option <99> = 5;"));
    let r = run(&inp, &["--redact-orphans"]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(!std::fs::read_to_string(&out).unwrap().contains("<99>"));
    assert!(r.stderr.contains("Warning [W22]:"), "{}", r.stderr);
}

#[test]
fn test_fail_on_completes_the_run_then_exits_3() {
    for quiet in [&[][..], &["-q"][..]] {
        let inp = inputs(&format!(
            "test_fail_on_completes_the_run_then_exits_3-{}",
            quiet.len()
        ));
        let mut args = vec!["--fail-on", "T4", "--fail-on", "W22", "--fail-on", "W3"];
        args.extend(quiet);
        let r = run(&inp, &args);
        assert_eq!(r.code, Some(3), "{}", r.stderr);
        assert_eq!(
            r.stderr.lines().last().unwrap(),
            "Error: --fail-on: W22 (1 occurrence), T4 (1 occurrence)"
        );
        assert!(inp.parent().unwrap().join("out/a.proto").exists());
        if !quiet.is_empty() {
            assert!(!r.stderr.contains("Warning ["), "{}", r.stderr);
        }
    }
}

#[test]
fn test_fail_on_a_code_that_did_not_occur_passes() {
    let inp = inputs("test_fail_on_a_code_that_did_not_occur_passes");
    assert_eq!(run(&inp, &["--fail-on", "W3"]).code, Some(0));
}

#[test]
fn test_fail_on_an_unknown_code_is_a_usage_error() {
    let r = run(
        &inputs("test_fail_on_an_unknown_code_is_a_usage_error"),
        &["--fail-on", "W99"],
    );
    assert_eq!(r.code, Some(2));
    assert!(
        r.stderr.contains("unknown warning code 'W99'"),
        "{}",
        r.stderr
    );
}

/// File options holding, undeclared, a group (field 7: <1> = 5, <2> = "x")
/// and a varint (field 99 = 5): each an orphan, with its value.
#[test]
fn test_an_undeclared_group_is_written_with_its_content() {
    if crate::via_text::on() {
        // As the original skipped it: the text format cannot keep a group,
        // protoc prints it as a message, read back as length-delimited,
        // with T1.
        eprintln!("skipped via text: protoc prints a group as a message");
        return;
    }
    let dir = tmp("test_an_undeclared_group_is_written_with_its_content");
    let d = dir.join("in");
    encode_fixture(
        "warning_codes/b.pb",
        "988a8898a4c4704dd9501740826382f8a9c1bb5a8119a8d408b9edf2211c9f16",
        &d.join("b.pb"),
    );
    let r = protoback([
        format!("-I{}", d.display()),
        "-O".into(),
        dir.join("out").display().to_string(),
        "--use-variant".into(),
        "descriptor".into(),
        "b.pb".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(dir.join("out/b.proto")).unwrap();
    assert!(
        text.contains(r#"///option <7> = { <1>: 5 <2>: "x" };"#),
        "{text}"
    );
    assert!(text.contains("///option <99> = 5;"), "{text}");
    assert_eq!(
        r.stderr.matches("Warning [W22]:").count(),
        2,
        "{}",
        r.stderr
    );
}
