// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Path patterns in -s/-p/-C (spec 0149): G3, prune-by-path eliminating
//! every physical candidate of an import, fires W1's "missing dependency";
//! G4, seed-by-path, a multi-FDP bundle seeds every FDP it holds.
//!
//! Ported from `test_path_patterns.py`, its command-line tests; names
//! lower-cased. Its unit tests (U2, U3) are in `protoback/src/pathmatch.rs`,
//! on `PathPatterns` and `fqdn_match`.

use crate::common::{compile_proto, compile_proto_multi, fixture, protoback, tmp, tools, Run};
use std::path::Path;

fn run(names: &[&str], out: &Path, extra: &[String]) -> Run {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(extra.iter().cloned());
    args.extend(names.iter().map(|s| s.to_string()));
    protoback(&args)
}

#[test]
fn test_g3_prune_by_path_eliminates_every_import_candidate() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_g3_prune_by_path_eliminates_every_import_candidate");
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    compile_proto(&pb, &["prune_glob_a.proto", "prune_glob_b.proto"]);
    // prune_glob_b.proto imports prune_glob_a.proto; pruning the import's
    // declared .proto path eliminates every physical extension candidate
    // for it (spec 0149 G3).
    let r = run(
        &["prune_glob_b.pb"],
        &out,
        &[
            format!("-I{}", pb.display()),
            "--prune".into(),
            "prune_glob_a.proto".into(),
        ],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(
        r.stderr
            .contains("missing dependency file:prune_glob_a.proto"),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("Traceback"));
    assert!(!out.join("prune_glob_a.proto").exists());
}

#[test]
fn test_g4_seed_by_path_multi_fdp_bundle() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_g4_seed_by_path_multi_fdp_bundle");
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    compile_proto_multi(
        &pb.join("bundle.protoset"),
        &[
            "prune_glob_a.proto",
            "prune_glob_b.proto",
            "prune_glob_c.proto",
        ],
    );
    let r = run(
        &["bundle.protoset"],
        &out,
        &[
            format!("-I{}", pb.display()),
            "--seed".into(),
            "bundle.protoset".into(),
        ],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    for f in [
        "prune_glob_a.proto",
        "prune_glob_b.proto",
        "prune_glob_c.proto",
    ] {
        assert!(out.join(f).exists(), "{f}");
    }
}
