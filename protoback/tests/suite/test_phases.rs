// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Regression tests for phases-level warning and error paths (spec 0041
//! TC-P*).
//!
//! Each test runs protoback as a command, the whole pipeline; the fixtures
//! compiled by protoc as needed.
//!
//! Ported from `test_phases.py`; its tests' names lower-cased (`test_P1_`
//! is `test_p1_`). The inputs its code built with Python's protobuf (P1's
//! self-dependent set, P5's two circular files, M2's set printed by
//! `text_format`) are the fixtures `phases/`, made once by that code
//! (`stash/0488/gen/test_phases.py`).

use crate::common::{
    compile_proto, compile_proto_multi, encode_fixture, fixture, protoback, tmp, tools, Run,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn run(pbs: &[PathBuf], out: &Path, extra: &[&str], include: &[PathBuf]) -> Run {
    let mut args: Vec<String> = vec!["--use-variant".into(), "descriptor".into()];
    args.push(format!("-I{}", fixture("").display()));
    args.extend(include.iter().map(|d| format!("-I{}", d.display())));
    args.push(format!("--proto-out={}", out.display()));
    args.extend(extra.iter().map(|s| s.to_string()));
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    protoback(&args)
}

/// A test's `pb` and `out` directories.
fn dirs(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    (dir, pb, out)
}

fn names(out: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![out.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                found.insert(p.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    found
}

/// A descriptor listing itself as a dependency: never a topological leaf,
/// it reaches the circular-dependency detector.
#[test]
fn test_p1_self_dependency_stripped() {
    let (dir, pb, out) = dirs("test_p1_self_dependency_stripped");
    let patched = pb.join("self_dep.pb");
    encode_fixture(
        "phases/self_dep.pb",
        "a6262d901834e979efe7cbffded56a945c90e7ea12ec014231e9721d30096ed4",
        &patched,
    );
    // Without the fixtures as sources: the self-dependency purely in the
    // descriptor. (The fixtures' directory stays on -I, as it did.)
    let empty = dir.join("empty_inc");
    std::fs::create_dir_all(&empty).unwrap();
    let r = run(&[patched], &out, &[], &[empty]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        r.stderr.contains("Circular dependency detected"),
        "{}",
        r.stderr
    );
}

/// Two files sharing exactly one symbol: "(1 symbol)" in the W3 line.
#[test]
fn test_p2_w3_singular_symbol() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_p2_w3_singular_symbol");
    let pbs = compile_proto(&pb, &["prune_duplicate_1.proto", "prune_duplicate_2.proto"]);
    let r = run(&pbs, &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(r.stderr.contains("(1 symbol)"), "{}", r.stderr);
    assert!(!r.stderr.contains("(1 symbols)"), "Must use singular form");
}

/// With --keep-duplicates, conflicting files are not pruned: no W3.
#[test]
fn test_p3_keep_duplicates_no_w3() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_p3_keep_duplicates_no_w3");
    let pbs = compile_proto(&pb, &["prune_duplicate_1.proto", "prune_duplicate_2.proto"]);
    let r = run(&pbs, &out, &["--keep-duplicates"], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("pruned") || !r.stderr.contains("duplicate symbols"),
        "W3 pruning must not fire when --keep-duplicates is active"
    );
}

/// A .pb file of garbage: a parse-failure message.
#[test]
fn test_p4_unparseable_pb() {
    let (_, pb, out) = dirs("test_p4_unparseable_pb");
    let corrupt = pb.join("corrupt.pb");
    std::fs::write(
        &corrupt,
        b"\xff\xfe\xfd this is not valid protobuf \x00\x01\x02",
    )
    .unwrap();
    let r = run(&[corrupt], &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        r.stderr.contains("Cannot parse") || r.stderr.contains("Skipping unparseable file"),
        "Expected a parse-failure message. stderr:\n{}",
        r.stderr
    );
}

/// Two descriptors importing each other: a circular-dependency warning.
#[test]
fn test_p5_circular_dependency() {
    let (_, pb, out) = dirs("test_p5_circular_dependency");
    let (a, b) = (pb.join("circ_a.pb"), pb.join("circ_b.pb"));
    encode_fixture(
        "phases/circ_a.pb",
        "c32f57716939cf6db762e23f75d4c5e37ee27f4d8a9f9bfc7b31605751a4786d",
        &a,
    );
    encode_fixture(
        "phases/circ_b.pb",
        "1fb8b8b069e515c512fead25380447536cd2728bb0f53b69d3525999355f1702",
        &b,
    );
    let r = run(&[a, b], &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        r.stderr.contains("Circular dependency detected"),
        "{}",
        r.stderr
    );
}

/// prune_base.proto compiled, run with `extra`.
fn base(test: &str, extra: &[&str]) -> Option<(Run, PathBuf)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let (_, pb, out) = dirs(test);
    let pbs = compile_proto(&pb, &["prune_base.proto"]);
    let r = run(&pbs, &out, extra, &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    Some((r, out))
}

#[test]
fn test_p6_prune_target_not_found() {
    let Some((r, _)) = base(
        "test_p6_prune_target_not_found",
        &["--prune", "file:nonexistent.proto"],
    ) else {
        return;
    };
    assert!(
        r.stderr
            .contains("Pruning target not found: file:nonexistent.proto"),
        "{}",
        r.stderr
    );
}

/// A near miss (one character short) suggests the real name.
#[test]
fn test_p7_prune_fuzzy_suggestion() {
    let Some((r, _)) = base(
        "test_p7_prune_fuzzy_suggestion",
        &["--prune", "file:prune_bse.proto"],
    ) else {
        return;
    };
    assert!(r.stderr.contains("Did you mean:"), "{}", r.stderr);
}

#[test]
fn test_p8_seed_not_found() {
    let Some((r, _)) = base(
        "test_p8_seed_not_found",
        &["--seed", "file:nonexistent.proto"],
    ) else {
        return;
    };
    assert!(
        r.stderr.contains("Seed not found: file:nonexistent.proto"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_p9_seed_fuzzy_suggestion() {
    let Some((r, _)) = base(
        "test_p9_seed_fuzzy_suggestion",
        &["--seed", "file:prune_bse.proto"],
    ) else {
        return;
    };
    assert!(r.stderr.contains("Did you mean:"), "{}", r.stderr);
}

/// A seed matching a --prune pattern is absent from the output (reported
/// not found or pruned: either; exit 0 and no file is what matters).
#[test]
fn test_p10_pruned_seed_skipped() {
    let Some((_, out)) = base(
        "test_p10_pruned_seed_skipped",
        &[
            "--prune",
            "file:prune_base.proto",
            "--seed",
            "file:prune_base.proto",
        ],
    ) else {
        return;
    };
    assert!(!out.join("prune_base.proto").exists());
}

/// A two-file set gives the same output files as the files apart.
fn same_as_single(test: &str, multi: impl FnOnce(&Path, &Path) -> PathBuf) {
    if !tools(&["protoc"]) {
        return;
    }
    let (dir, pb, _) = dirs(test);
    let (single_out, multi_out) = (dir.join("out_single"), dir.join("out_multi"));
    std::fs::create_dir_all(&single_out).unwrap();
    std::fs::create_dir_all(&multi_out).unwrap();
    let singles = compile_proto(&pb, &["phone_number.proto", "address_book.proto"]);
    let multi = multi(&dir, &pb);
    let rs = run(&singles, &single_out, &[], &[]);
    let rm = run(&[multi], &multi_out, &[], &[]);
    assert_eq!(rs.code, Some(0), "single run failed:\n{}", rs.stderr);
    assert_eq!(rm.code, Some(0), "multi run failed:\n{}", rm.stderr);
    assert_eq!(names(&single_out), names(&multi_out));
}

/// address_book.proto imports phone_number.proto: --include_imports
/// bundles both into one set.
#[test]
fn test_m1_multi_fdp_binary_fds() {
    same_as_single("test_m1_multi_fdp_binary_fds", |_, pb| {
        compile_proto_multi(&pb.join("multi.pb"), &["address_book.proto"])
    });
}

/// The same set as text, as Python's `text_format` printed it.
#[test]
fn test_m2_multi_fdp_text_fds() {
    same_as_single("test_m2_multi_fdp_text_fds", |dir, _| {
        let textpb = dir.join("multi.textpb");
        std::fs::copy(fixture("phases/multi.textpb"), &textpb).unwrap();
        textpb
    });
}

#[test]
fn test_m3_single_fdp_binary_regression() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_m3_single_fdp_binary_regression");
    let pbs = compile_proto(&pb, &["phone_number.proto"]);
    let r = run(&pbs, &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        out.join("phone_number.proto").exists(),
        "Expected phone_number.proto in output"
    );
}

/// Two sets sharing a file: accepted, one copy processed.
#[test]
fn test_m4_fdp_name_collision_first_wins() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_m4_fdp_name_collision_first_wins");
    let a = compile_proto_multi(&pb.join("multi_a.pb"), &["address_book.proto"]);
    let b = compile_proto_multi(&pb.join("multi_b.pb"), &["address_book.proto"]);
    let r = run(&[a, b], &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        out.join("phone_number.proto").exists(),
        "Expected phone_number.proto in output"
    );
    assert!(
        out.join("address_book.proto").exists(),
        "Expected address_book.proto in output"
    );
}

/// A normal -O run writes a minimal buf.yaml at the output root (spec
/// 0146).
#[test]
fn test_b1_buf_yaml_written_at_out_repo_root() {
    let Some((_, out)) = base("test_b1_buf_yaml_written_at_out_repo_root", &[]) else {
        return;
    };
    let content = std::fs::read_to_string(out.join("buf.yaml")).unwrap();
    assert!(
        content.contains("version: v2") && content.contains("path: ."),
        "{content}"
    );
}

/// --dry-run writes no files at all, buf.yaml included.
#[test]
fn test_b2_buf_yaml_skipped_on_dry_run() {
    let Some((_, out)) = base("test_b2_buf_yaml_skipped_on_dry_run", &["--dry-run"]) else {
        return;
    };
    assert!(!out.join("buf.yaml").exists());
}

/// A buf.yaml already in the output directory is left untouched.
#[test]
fn test_b3_existing_buf_yaml_not_overwritten() {
    if !tools(&["protoc"]) {
        return;
    }
    let (_, pb, out) = dirs("test_b3_existing_buf_yaml_not_overwritten");
    let custom = "# hand-customized\nversion: v2\nlint:\n  use:\n    - DEFAULT\n";
    std::fs::write(out.join("buf.yaml"), custom).unwrap();
    let pbs = compile_proto(&pb, &["prune_base.proto"]);
    let r = run(&pbs, &out, &[], &[]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert_eq!(
        std::fs::read_to_string(out.join("buf.yaml")).unwrap(),
        custom
    );
}
