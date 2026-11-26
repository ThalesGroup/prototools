// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Glob patterns in --seed and --prune (spec 0074): T1, a file pruned at
//! load time does not win a symbol conflict; T2, a glob --prune, `*` not
//! crossing `/`; T3, a glob --seed; T4, a glob --prune with a plain --seed.
//!
//! Ported from `test_prune_seed_patterns.py`, its command-line tests; names
//! lower-cased. Its unit tests (T2's and T5's, `fqdn_match`) are in
//! `protoback/src/pathmatch.rs`.

use crate::common::{compile_proto, compile_proto_multi, fixture, protoback, tmp, tools, Run};
use std::path::{Path, PathBuf};

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

fn dirs(test: &str) -> Option<(PathBuf, PathBuf)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    Some((pb, out))
}

const GLOB: &[&str] = &[
    "prune_glob_a.proto",
    "prune_glob_b.proto",
    "prune_glob_c.proto",
];

/// Pruning the winner at load time lets the loser survive, with no
/// spurious auto-prune warning.
#[test]
fn test_t1_load_time_prune_no_conflict_win() {
    let Some((pb, out)) = dirs("test_t1_load_time_prune_no_conflict_win") else {
        return;
    };
    let pbs = compile_proto(
        &pb,
        &["prune_conflict_winner.proto", "prune_conflict_loser.proto"],
    );
    let r = run(&pbs, &out, &["--prune", "file:prune_conflict_winner.proto"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(!out.join("prune_conflict_winner.proto").exists());
    assert!(out.join("prune_conflict_loser.proto").exists());
    assert!(
        !r.stderr.contains("prune_conflict_loser") || !r.stderr.contains("duplicate"),
        "{}",
        r.stderr
    );
}

/// file:prune_glob_b* prunes prune_glob_b.proto, not a or c.
#[test]
fn test_t2_glob_prune_single_segment() {
    let Some((pb, out)) = dirs("test_t2_glob_prune_single_segment") else {
        return;
    };
    let pbs = compile_proto(&pb, GLOB);
    let r = run(&pbs, &out, &["--prune", "file:prune_glob_b*"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(!out.join("prune_glob_b.proto").exists());
    assert!(out.join("prune_glob_a.proto").exists());
    assert!(out.join("prune_glob_c.proto").exists());
}

/// file:prune_glob_b* as seed renders b and its import a; c excluded.
#[test]
fn test_t3_glob_seed() {
    let Some((pb, out)) = dirs("test_t3_glob_seed") else {
        return;
    };
    let multi = compile_proto_multi(&pb.join("all.pb"), GLOB);
    let r = run(&[multi], &out, &["--seed", "file:prune_glob_b*"]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_glob_b.proto").exists());
    assert!(out.join("prune_glob_a.proto").exists());
    assert!(!out.join("prune_glob_c.proto").exists());
}

/// --seed c --prune 'file:prune_glob_a*': c rendered, a excluded at load
/// time.
#[test]
fn test_t4_glob_prune_with_plain_seed() {
    let Some((pb, out)) = dirs("test_t4_glob_prune_with_plain_seed") else {
        return;
    };
    let pbs = compile_proto(&pb, GLOB);
    let r = run(
        &pbs,
        &out,
        &[
            "--seed",
            "file:prune_glob_c.proto",
            "--prune",
            "file:prune_glob_a*",
        ],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(!out.join("prune_glob_a.proto").exists());
    assert!(out.join("prune_glob_c.proto").exists());
}
