// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! --filter-config (spec 0149), wired to the command line: T1, alone, as
//! -s/-p; T2, unioned with -s/-p; T3, a missing file a clean error; T4, an
//! unknown key a clean usage error; G6, a bare entry a path pattern; G7, a
//! colon value of an unknown prefix a clean usage error.
//!
//! Ported from `test_filter_config.py`, its command-line tests; names
//! lower-cased. Its unit tests (U1, `filter_config.load`) are in
//! `protoback/src/main.rs`.

use crate::common::{compile_proto, fixture, protoback, tmp, tools, write, Run};
use std::path::{Path, PathBuf};

fn run(pbs: &[String], out: &Path, extra: &[String]) -> Run {
    let mut args: Vec<String> = vec![
        "--use-variant".into(),
        "descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
    ];
    args.extend(extra.iter().cloned());
    args.extend(pbs.iter().cloned());
    protoback(&args)
}

/// The test's directory, `pb` holding `protos` compiled, and `out`.
fn setup(test: &str, protos: &[&str]) -> Option<(PathBuf, PathBuf, PathBuf, Vec<String>)> {
    if !tools(&["protoc"]) {
        return None;
    }
    let dir = tmp(test);
    let (pb, out) = (dir.join("pb"), dir.join("out"));
    std::fs::create_dir_all(&pb).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let pbs = compile_proto(&pb, protos)
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    Some((dir, pb, out, pbs))
}

const GLOB: &[&str] = &[
    "prune_glob_a.proto",
    "prune_glob_b.proto",
    "prune_glob_c.proto",
];

#[test]
fn test_t1_filter_config_equivalent_to_cli_seed() {
    let Some((dir, _, out, pbs)) = setup("test_t1_filter_config_equivalent_to_cli_seed", GLOB)
    else {
        return;
    };
    let config = dir.join("filter.yaml");
    write(&config, "seed:\n  - file:prune_glob_b*\n");
    let r = run(
        &pbs,
        &out,
        &["--filter-config".into(), config.display().to_string()],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_glob_b.proto").exists());
    assert!(out.join("prune_glob_a.proto").exists());
    assert!(!out.join("prune_glob_c.proto").exists());
}

/// The config seeds a, the command line c; b, seeded by neither, not
/// reachable.
#[test]
fn test_t2_filter_config_unions_with_cli_flags() {
    let Some((dir, _, out, pbs)) = setup("test_t2_filter_config_unions_with_cli_flags", GLOB)
    else {
        return;
    };
    let config = dir.join("filter.yaml");
    write(&config, "seed:\n  - file:prune_glob_a.proto\n");
    let r = run(
        &pbs,
        &out,
        &[
            "-C".into(),
            config.display().to_string(),
            "--seed".into(),
            "file:prune_glob_c.proto".into(),
        ],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_glob_a.proto").exists());
    assert!(out.join("prune_glob_c.proto").exists());
    assert!(!out.join("prune_glob_b.proto").exists());
}

#[test]
fn test_t3_filter_config_missing_file() {
    let Some((dir, _, out, pbs)) = setup(
        "test_t3_filter_config_missing_file",
        &["prune_glob_a.proto"],
    ) else {
        return;
    };
    let r = run(
        &pbs,
        &out,
        &[
            "--filter-config".into(),
            dir.join("nonexistent.yaml").display().to_string(),
        ],
    );
    assert_ne!(r.code, Some(0));
    assert!(!r.stderr.contains("Traceback"));
}

#[test]
fn test_t4_filter_config_unknown_key() {
    let Some((dir, _, out, pbs)) =
        setup("test_t4_filter_config_unknown_key", &["prune_glob_a.proto"])
    else {
        return;
    };
    let config = dir.join("filter.yaml");
    write(&config, "prunes:\n  - file:bar.proto\n");
    let r = run(
        &pbs,
        &out,
        &["--filter-config".into(), config.display().to_string()],
    );
    assert_ne!(r.code, Some(0));
    assert!(
        r.stderr.contains("unknown filter-config key"),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("Traceback"));
}

/// No `file:` prefix: a literal path pattern (G2), matched against the
/// root-relative path of the physical .pb; the files given relative to an
/// extra -I root for that.
#[test]
fn test_g6_bare_filter_config_entry_resolves_to_path() {
    let Some((dir, pb, out, _)) = setup("test_g6_bare_filter_config_entry_resolves_to_path", GLOB)
    else {
        return;
    };
    let config = dir.join("filter.yaml");
    write(&config, "seed:\n  - prune_glob_b.pb\n");
    let names: Vec<String> = ["prune_glob_a.pb", "prune_glob_b.pb", "prune_glob_c.pb"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let r = run(
        &names,
        &out,
        &[
            format!("-I{}", pb.display()),
            "--filter-config".into(),
            config.display().to_string(),
        ],
    );
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(out.join("prune_glob_b.proto").exists());
    assert!(out.join("prune_glob_a.proto").exists());
    assert!(!out.join("prune_glob_c.proto").exists());
}

#[test]
fn test_g7_unrecognized_prefix_raises_clean_usage_error() {
    let Some((_, _, out, pbs)) = setup(
        "test_g7_unrecognized_prefix_raises_clean_usage_error",
        &["prune_glob_a.proto"],
    ) else {
        return;
    };
    let r = run(&pbs, &out, &["--prune".into(), "fille:foo.proto".into()]);
    assert_ne!(r.code, Some(0));
    assert!(
        r.stderr.contains("fille") && r.stderr.contains("path:"),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("Traceback"));
}
