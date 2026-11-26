// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Regression tests for the summoning algorithm (spec 0046).
//!
//! Each test compiles fixture .proto files with protoc, then runs protoback
//! and checks which files appear in the output directory. The fixtures are
//! named tc<N>_*.proto; TC-5 reuses the bridge_lex_*.proto ones.
//!
//! Ported from `test_summoning.py`; its tests' names lower-cased
//! (`test_TC1_` is `test_tc1_`).

use crate::common::{compile_proto, fixture, protoback, tmp, tools, Run};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The fixtures compiled, run with `seeds` and `prunings`; the run, and the
/// names of the .proto files written.
fn summon(
    test: &str,
    protos: &[&str],
    seeds: &[&str],
    prunings: &[&str],
) -> Option<(Run, BTreeSet<String>)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pbs = compile_proto(&pb, protos);
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
    ];
    for s in seeds {
        args.extend(["--seed".into(), s.to_string()]);
    }
    for p in prunings {
        args.extend(["--prune".into(), p.to_string()]);
    }
    args.extend(pbs.iter().map(|p| p.display().to_string()));
    let r = protoback(&args);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    Some((r, written(&out)))
}

/// The names of the .proto files written under `out`.
fn written(out: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut stack: Vec<PathBuf> = vec![out.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                names.insert(p.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    names
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// A directly imports C; TA has a field of type TC: only A and C written.
#[test]
fn test_tc1_direct_import_no_bridge() {
    let Some((_, w)) = summon(
        "test_tc1_direct_import_no_bridge",
        &["tc1_c.proto", "tc1_a.proto"],
        &["desc:.tc1.TA"],
        &[],
    ) else {
        return;
    };
    assert_eq!(w, set(&["tc1_a.proto", "tc1_c.proto"]));
}

/// A imports B imports C; TA has a field of type TC: B written as a bridge,
/// though nothing in it is seeded.
#[test]
fn test_tc2_one_hop_bridge() {
    let Some((_, w)) = summon(
        "test_tc2_one_hop_bridge",
        &["tc2_c.proto", "tc2_b.proto", "tc2_a.proto"],
        &["desc:.tc2.TA"],
        &[],
    ) else {
        return;
    };
    assert!(
        w.contains("tc2_b.proto"),
        "tc2_b.proto must be written as a bridge"
    );
    assert_eq!(w, set(&["tc2_a.proto", "tc2_b.proto", "tc2_c.proto"]));
}

/// A→B→D→C; TA references TC: B and D both written as bridges.
#[test]
fn test_tc3_two_hop_bridge() {
    let Some((_, w)) = summon(
        "test_tc3_two_hop_bridge",
        &["tc3_c.proto", "tc3_d.proto", "tc3_b.proto", "tc3_a.proto"],
        &["desc:.tc3.TA"],
        &[],
    ) else {
        return;
    };
    assert_eq!(
        w,
        set(&["tc3_a.proto", "tc3_b.proto", "tc3_d.proto", "tc3_c.proto"])
    );
}

/// Seeding only Leaf (scalar fields only) does not pull in tc4_user.proto.
#[test]
fn test_tc4_no_spurious_summoning() {
    let Some((_, w)) = summon(
        "test_tc4_no_spurious_summoning",
        &["tc4_leaf.proto", "tc4_user.proto"],
        &["desc:.tc4.Leaf"],
        &[],
    ) else {
        return;
    };
    assert!(w.contains("tc4_leaf.proto"));
    assert!(
        !w.contains("tc4_user.proto"),
        "tc4_user.proto imports Leaf but is not reachable: must not be written"
    );
}

/// A imports B1 and B2, both importing C; A references LC: two paths of
/// length 2, the lexicographically smaller, through B1, wins.
#[test]
fn test_tc5_lex_tie_breaking() {
    let Some((_, w)) = summon(
        "test_tc5_lex_tie_breaking",
        &[
            "bridge_lex_c.proto",
            "bridge_lex_b1.proto",
            "bridge_lex_b2.proto",
            "bridge_lex_a.proto",
        ],
        &["desc:.bridgelex.LexA"],
        &[],
    ) else {
        return;
    };
    assert!(w.contains("bridge_lex_a.proto"));
    assert!(w.contains("bridge_lex_c.proto"));
    assert!(
        w.contains("bridge_lex_b1.proto"),
        "B1 is lex-smaller: must be the chosen bridge"
    );
    assert!(
        !w.contains("bridge_lex_b2.proto"),
        "B2 is lex-larger: must not be written"
    );
}

/// A imports B imports C; TA has fields of both TB and TC: B both a direct
/// dependency and a bridge; all three written.
#[test]
fn test_tc6_shared_bridge() {
    let Some((_, w)) = summon(
        "test_tc6_shared_bridge",
        &["tc6_c.proto", "tc6_b.proto", "tc6_a.proto"],
        &["desc:.tc6.TA"],
        &[],
    ) else {
        return;
    };
    assert_eq!(w, set(&["tc6_a.proto", "tc6_b.proto", "tc6_c.proto"]));
}

/// A file that only imports a seeded file is not itself written.
#[test]
fn test_tc7_import_only_not_written() {
    let Some((_, w)) = summon(
        "test_tc7_import_only_not_written",
        &["tc7_s.proto", "tc7_u.proto"],
        &["desc:.tc7.TS"],
        &[],
    ) else {
        return;
    };
    assert!(w.contains("tc7_s.proto"));
    assert!(
        !w.contains("tc7_u.proto"),
        "tc7_u.proto only imports the seeded file: must not be written"
    );
}

/// A file both a seed and explicitly pruned is not written.
#[test]
fn test_tc8_pruned_seed_not_written() {
    let Some((_, w)) = summon(
        "test_tc8_pruned_seed_not_written",
        &["tc8_p.proto", "tc8_q.proto"],
        &[],
        &["file:tc8_p.proto"],
    ) else {
        return;
    };
    assert!(w.contains("tc8_q.proto"));
    assert!(
        !w.contains("tc8_p.proto"),
        "tc8_p.proto is explicitly pruned: must not be written even though it was loaded as a seed"
    );
}
