// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Regression tests for pruned-file dependency handling (spec 0041).
//!
//! Before the fix, a file that imported a pruned file crashed in phase 7
//! with "Node ... not initialized with message", for --prune-triggered
//! pruning and duplicate-symbol pruning alike.
//!
//! Fixtures: prune_base.proto (prune_test.Base), prune_importer.proto
//! (imports it, prune_test.Importer), prune_duplicate_1.proto and
//! prune_duplicate_2.proto (both prune_dup.SharedMsg), prune_dup_importer.proto
//! (imports the second, prune_dup_consumer.Consumer).
//!
//! Ported from `test_pruned_dependency.py`. The input of its last test,
//! built with Python's protobuf, is the `#@prototext` fixture
//! `pruned_dependency/set.pb`, made once by that code
//! (`protoback/tests/fixtures/gen/test_pruned_dependency.py`).

use crate::common::{compile_proto, encode_fixture, fixture, protoback, tmp, tools, Run};
use std::path::{Path, PathBuf};

fn run(pbs: &[PathBuf], out: &Path, extra: &[&str], include: &[PathBuf]) -> Run {
    let mut args: Vec<String> = vec!["--use-variant".into(), "descriptor".into()];
    args.extend(include.iter().map(|d| format!("-I{}", d.display())));
    args.push(format!("--proto-out={}", out.display()));
    args.extend(extra.iter().map(|s| s.to_string()));
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    protoback(&args)
}

fn dirs(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    (dir, pb, out)
}

/// A file importing an explicitly pruned file renders.
#[test]
fn test_explicit_prune_no_crash() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_explicit_prune_no_crash");
    let pbs = compile_proto(&pb, &["prune_base.proto", "prune_importer.proto"]);
    let r = run(
        &pbs,
        &out,
        &["--prune", "file:prune_base.proto"],
        &[fixture("")],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_importer.proto").exists());
    assert!(!out.join("prune_base.proto").exists());
}

/// A file importing a duplicate-pruned file renders. Which duplicate wins
/// is not fixed: exactly one survives.
#[test]
fn test_duplicate_prune_no_crash() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_duplicate_prune_no_crash");
    let pbs = compile_proto(
        &pb,
        &[
            "prune_duplicate_1.proto",
            "prune_duplicate_2.proto",
            "prune_dup_importer.proto",
        ],
    );
    let r = run(&pbs, &out, &[], &[fixture("")]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_dup_importer.proto").exists());
    let winners = ["prune_duplicate_1.proto", "prune_duplicate_2.proto"]
        .iter()
        .filter(|f| out.join(f).exists())
        .count();
    assert_eq!(
        winners, 1,
        "Expected exactly one of prune_duplicate_1/2 in output, got {winners}"
    );
    // W3 for whichever was pruned; TC-R2: no W5 for either.
    assert!(
        r.stderr.contains("pruned") && r.stderr.contains("duplicate symbols"),
        "{}",
        r.stderr
    );
    for f in ["prune_duplicate_1.proto", "prune_duplicate_2.proto"] {
        assert!(
            !r.stderr.contains(&format!("missing dependency file:{f}")),
            "W5 for a pruned duplicate must be suppressed"
        );
    }
}

/// TC-R1: no "missing dependency" warning for an explicitly pruned file.
#[test]
fn test_explicit_prune_no_w5() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_explicit_prune_no_w5");
    let pbs = compile_proto(&pb, &["prune_base.proto", "prune_importer.proto"]);
    let r = run(
        &pbs,
        &out,
        &["--prune", "file:prune_base.proto"],
        &[fixture("")],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr
            .contains("missing dependency file:prune_base.proto"),
        "W5 for an explicitly pruned file must be suppressed"
    );
}

/// TC-R3: a dependency missing at loading appears as a single W5 line.
#[test]
fn test_w1_loading_miss_appears_as_w5() {
    if !tools(&["protoc"]) {
        return;
    }
    let (dir, pb, out) = dirs("test_w1_loading_miss_appears_as_w5");
    // prune_importer alone, prune_base.proto on no include path.
    let pbs = compile_proto(&pb, &["prune_importer.proto"]);
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let r = run(&pbs, &out, &[], &[empty]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let w5 = r
        .stderr
        .lines()
        .filter(|l| l.contains("missing dependency file:prune_base.proto"))
        .count();
    assert_eq!(
        w5, 1,
        "Expected exactly one W5 squashed line for prune_base.proto.\nstderr:\n{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("missing file 'prune_base.proto'"),
        "W1 must not produce a separate line: it must merge into the W5 counter"
    );
}

/// TC-R4: an import stripped before the pool shifts the ones after it; the
/// public and weak indices must follow them (spec 0424 S1). importer.proto
/// imports missing.proto (never provided), then weak.proto as weak, then
/// public.proto as public.
#[test]
fn test_stripped_import_keeps_public_and_weak_on_their_files() {
    let dir = tmp("test_stripped_import_keeps_public_and_weak_on_their_files");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let pb = dir.join("set.pb");
    encode_fixture(
        "pruned_dependency/set.pb",
        "a768605d77fd8f481966451a04cbe69795e7f4097f30f608aa7f2d62984775dd",
        &pb,
    );
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let r = run(&[pb], &out, &[], &[empty]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    let text = std::fs::read_to_string(out.join("importer.proto")).unwrap();
    let imports: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("import"))
        .map(str::trim)
        .collect();
    assert!(imports.contains(&r#"import weak "weak.proto";"#), "{text}");
    assert!(
        imports.contains(&r#"import public "public.proto";"#),
        "{text}"
    );
    assert!(imports.contains(&r#"///import "missing.proto";"#), "{text}");
    assert!(
        !imports
            .iter()
            .any(|l| l.starts_with(r#"import public "weak"#)),
        "{text}"
    );
}
