// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0243: a blob is a root, and the defaults fill the rest.
//!
//! - --schema-db-out FILE.desc implies -O FILE-stem/proto (S1), which
//!   --no-proto-out suppresses (S2);
//! - -I accepts a blob file and reads it as the descriptors inside it (S5);
//! - no DESCRIPTOR_FILES means '.' (S13).
//!
//! Ported from `test_blob_root_and_defaults.py`, its command-line tests.
//! Its two members, built with Python's protobuf, importing nothing, are the
//! `#@prototext` fixtures `blob_root/` (`protoback/tests/fixtures/gen/`), the blob
//! assembled from them as its `_write_blob` did: a header, the members
//! joined by NULs, a footer. Its unit test is in `protoback/src/complete.rs`.

use crate::common::{encoded, protoback, tmp, Run};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The members' bytes, checked against the original's.
fn members() -> [(String, Vec<u8>); 2] {
    let read = encoded;
    [
        (
            "first.proto".into(),
            read(
                "blob_root/first.pb",
                "c77e19e18ba49e0ed386e71e52ea1a726913a1c069ddc3416778afc602324996",
            ),
        ),
        (
            "extra/second.proto".into(),
            read(
                "blob_root/second.pb",
                "3450fac04dfbfbf7d33428b9e66887d798dbd3dc33f7d499b24682b8daa3b3fd",
            ),
        ),
    ]
}

/// The members with filler around them (`_write_blob`).
fn write_blob(path: &Path) -> PathBuf {
    let bodies: Vec<Vec<u8>> = members().into_iter().map(|(_, b)| b).collect();
    let body = bodies.join(&0u8);
    std::fs::write(
        path,
        [b"HEADER\x00".as_slice(), &body, b"\x00FOOTER"].concat(),
    )
    .unwrap();
    path.to_path_buf()
}

/// The same members, extracted, as protoscan --proto_out writes them
/// (`_write_dir`).
fn write_dir(root: &Path) -> PathBuf {
    for (name, bytes) in members() {
        let out = root.join(Path::new(&name).with_extension("pb"));
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, bytes).unwrap();
    }
    root.to_path_buf()
}

/// `--use-variant=descriptor` on every run, as the original: the members
/// import nothing, but descriptor.proto is wanted in the input set.
fn run(args: &[String]) -> Run {
    let mut all = vec!["--use-variant=descriptor".to_string()];
    all.extend(args.iter().cloned());
    protoback(&all)
}

fn protos(root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "proto") {
                found.insert(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    found
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_schema_db_out_implies_proto_out() {
    let dir = tmp("test_schema_db_out_implies_proto_out");
    let blob = write_blob(&dir.join("blob"));
    let r = run(&[
        format!("--schema-db-out={}", dir.join("db.desc").display()),
        format!("-I{}", blob.display()),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(
        protos(&dir.join("db/proto")),
        set(&["first.proto", "extra/second.proto"])
    );
}

#[test]
fn test_no_proto_out_suppresses_the_implied_proto_out() {
    let dir = tmp("test_no_proto_out_suppresses_the_implied_proto_out");
    let blob = write_blob(&dir.join("blob"));
    let r = run(&[
        format!("--schema-db-out={}", dir.join("db.desc").display()),
        "--no-proto-out".into(),
        format!("-I{}", blob.display()),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(dir.join("db.desc").exists());
    assert!(!dir.join("db/proto").exists());
}

#[test]
fn test_no_proto_out_with_explicit_proto_out_is_a_usage_error() {
    let dir = tmp("test_no_proto_out_with_explicit_proto_out_is_a_usage_error");
    let r = run(&[
        format!("--schema-db-out={}", dir.join("db.desc").display()),
        "--no-proto-out".into(),
        format!("-O{}", dir.join("out").display()),
        dir.join("missing.pb").display().to_string(),
    ]);
    assert_ne!(r.code, Some(0));
    assert!(
        r.stderr.contains("--no-proto-out contradicts"),
        "{}",
        r.stderr
    );
}

/// A blob root produces exactly what the extracted directory does.
#[test]
fn test_desc_root_may_be_a_blob() {
    let dir = tmp("test_desc_root_may_be_a_blob");
    let blob = write_blob(&dir.join("blob"));
    let extracted = write_dir(&dir.join("extracted"));
    let (from_blob, from_dir) = (dir.join("from_blob"), dir.join("from_dir"));
    assert_eq!(
        run(&[
            format!("-O{}", from_blob.display()),
            format!("-I{}", blob.display())
        ])
        .code,
        Some(0)
    );
    assert_eq!(
        run(&[
            format!("-O{}", from_dir.display()),
            format!("-I{}", extracted.display())
        ])
        .code,
        Some(0)
    );
    assert_eq!(protos(&from_blob), protos(&from_dir));
    for name in protos(&from_blob) {
        assert_eq!(
            std::fs::read_to_string(from_blob.join(&name)).unwrap(),
            std::fs::read_to_string(from_dir.join(&name)).unwrap(),
            "{name}"
        );
    }
}

/// -p matches a blob member on its root-relative path, as for a directory.
#[test]
fn test_blob_member_paths_are_the_fdp_names() {
    let dir = tmp("test_blob_member_paths_are_the_fdp_names");
    let blob = write_blob(&dir.join("blob"));
    let out = dir.join("out");
    let r = run(&[
        format!("-O{}", out.display()),
        format!("-I{}", blob.display()),
        "-p".into(),
        "extra/second.pb".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(protos(&out), set(&["first.proto"]));
}

#[test]
fn test_a_named_blob_member_loads_alone() {
    let dir = tmp("test_a_named_blob_member_loads_alone");
    let blob = write_blob(&dir.join("blob"));
    let out = dir.join("out");
    let r = run(&[
        format!("-O{}", out.display()),
        format!("-I{}", blob.display()),
        "extra/second.pb".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(protos(&out), set(&["extra/second.proto"]));
}

#[test]
fn test_blob_with_no_descriptors_is_an_error() {
    let dir = tmp("test_blob_with_no_descriptors_is_an_error");
    let junk = dir.join("junk.bin");
    std::fs::write(&junk, (0..=255u8).cycle().take(1024).collect::<Vec<u8>>()).unwrap();
    let r = run(&[
        format!("-O{}", dir.join("out").display()),
        format!("-I{}", junk.display()),
    ]);
    assert_ne!(r.code, Some(0));
    assert!(
        r.stderr.contains("holds no FileDescriptorProto"),
        "{}",
        r.stderr
    );
}

#[test]
fn test_no_arguments_means_dot() {
    let dir = tmp("test_no_arguments_means_dot");
    let extracted = write_dir(&dir.join("extracted"));
    let (implied, spelled) = (dir.join("implied"), dir.join("spelled"));
    assert_eq!(
        run(&[
            format!("-O{}", implied.display()),
            format!("-I{}", extracted.display())
        ])
        .code,
        Some(0)
    );
    assert_eq!(
        run(&[
            format!("-O{}", spelled.display()),
            format!("-I{}", extracted.display()),
            ".".into()
        ])
        .code,
        Some(0)
    );
    assert_eq!(protos(&implied), protos(&spelled));
    assert!(!protos(&implied).is_empty());
}
