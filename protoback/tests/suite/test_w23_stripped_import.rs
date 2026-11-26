// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! W23: an import of a pruned file stripped from its importer (spec 0438
//! S1).
//!
//! Apart from W3 (a duplicate pruned), so that `--fail-on W3` asserts that no
//! duplicate was resolved by load order, whatever a prune pattern strips.
//!
//! Ported from `test_w23_stripped_import.py`.

use crate::common::{protoback, protoc, tmp, tools, write, Run};

const BASE: &str = "syntax = \"proto2\";\npackage p;\nmessage Base { optional int32 x = 1; }\n";
const USER: &str = "syntax = \"proto2\";\npackage p;\nimport \"base.proto\";\n\
                    message User { optional int32 y = 1; }\n";

fn run(name: &str, flags: &[&str]) -> Option<Run> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(name);
    let (src, inp) = (dir.join("src"), dir.join("in"));
    write(&src.join("base.proto"), BASE);
    write(&src.join("user.proto"), USER);
    std::fs::create_dir_all(&inp).unwrap();
    protoc([
        format!("-I{}", src.display()),
        "--include_imports".into(),
        format!("--descriptor_set_out={}", inp.join("set.pb").display()),
        "user.proto".into(),
    ]);
    let mut args: Vec<std::ffi::OsString> =
        ["--use-variant", "descriptor", "-p", "file:base.proto"]
            .iter()
            .map(Into::into)
            .collect();
    args.extend(flags.iter().map(Into::into));
    args.push(format!("-I{}", inp.display()).into());
    args.extend(["-O".into(), dir.join("out").into_os_string()]);
    Some(protoback(args))
}

#[test]
fn test_a_stripped_import_of_a_pruned_file_is_w23_not_w3() {
    let Some(run) = run("test_a_stripped_import_of_a_pruned_file_is_w23_not_w3", &[]) else {
        return;
    };
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr
            .contains(r#"Warning [W23]: stripped pruned dependency "base.proto" from user.proto"#),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("[W3]"), "{}", run.stderr);
}

#[test]
fn test_fail_on_w3_ignores_a_stripped_import() {
    let Some(run) = run(
        "test_fail_on_w3_ignores_a_stripped_import",
        &["--fail-on", "W3"],
    ) else {
        return;
    };
    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

#[test]
fn test_fail_on_w23_counts_it() {
    let Some(run) = run("test_fail_on_w23_counts_it", &["--fail-on", "W23"]) else {
        return;
    };
    assert_eq!(run.code, Some(3), "{}", run.stderr);
    assert!(
        run.stderr.contains("--fail-on: W23 (1 occurrence)"),
        "{}",
        run.stderr
    );
}
