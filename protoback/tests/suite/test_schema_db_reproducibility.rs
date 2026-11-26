// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Compiling the same input twice yields identical artifacts (spec 0177).
//!
//! Both schema-DB sidecars used to differ on every build: `hopcroft.rkyv`,
//! because `graph::build` numbered nodes by iterating a `HashMap` and
//! Hopcroft's refinement allocated block IDs while iterating another one;
//! `index.rkyv`, because rkyv lays an `ArchivedHashMap` out in the source
//! map's iteration order, and `std::HashMap`'s `RandomState` is seeded per
//! process. What this guards is content-addressability: verifying, caching
//! or deduplicating a DB by digest, and diffing two DBs.
//!
//! Each build runs in its own process: `RandomState` is per process, so an
//! in-process loop would have passed even before the fix. The schema must be
//! large enough for Hopcroft to split blocks, where the block-numbering
//! nondeterminism lived: hence the well-known imports, whose
//! `--include_imports` pulls in descriptor.proto and friends.
//!
//! Ported from `test_schema_db_reproducibility.py`; its test's name
//! lower-cased.

use crate::common::{protoback, tmp, tools, write};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const BUILDS: usize = 4;

const PROTO: &str = r#"syntax = "proto3";
package repro;
import "google/protobuf/descriptor.proto";
import "google/protobuf/struct.proto";
import "google/protobuf/timestamp.proto";
message Holder {
  google.protobuf.FileDescriptorSet fds = 1;
  google.protobuf.Struct payload = 2;
  google.protobuf.Timestamp when = 3;
  repeated google.protobuf.FileDescriptorProto files = 4;
}
"#;

/// protoback in a fresh process; the schema DB's sidecar directory.
fn build_db(dir: &Path, pb: &Path, tag: &str) -> PathBuf {
    let db = dir.join(tag).join("repro.desc");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        format!("--proto-out={}", dir.join(tag).join("out").display()),
        format!("--schema-db-out={}", db.display()),
        pb.display().to_string(),
    ]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    dir.join(tag).join("repro")
}

/// Four independent builds of one input agree byte for byte on both rkyv
/// sidecars (spec 0177 S1-S3).
#[test]
fn test_r1_schema_db_sidecars_are_byte_reproducible() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_r1_schema_db_sidecars_are_byte_reproducible");
    write(&dir.join("src/repro.proto"), PROTO);
    let pb = dir.join("repro.pb");
    let o = std::process::Command::new("protoc")
        .current_dir(&dir)
        .arg(format!("-I{}", dir.join("src").display()))
        .arg("--include_imports")
        .arg(format!("--descriptor_set_out={}", pb.display()))
        .arg("repro.proto")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dirs: Vec<PathBuf> = (0..BUILDS)
        .map(|i| build_db(&dir, &pb, &format!("b{i}")))
        .collect();
    for name in ["hopcroft.rkyv", "index.rkyv"] {
        let digests: BTreeSet<String> = dirs
            .iter()
            .map(|d| format!("{:x}", Sha256::digest(std::fs::read(d.join(name)).unwrap())))
            .collect();
        assert_eq!(
            digests.len(),
            1,
            "{name} differs across {BUILDS} builds of identical input: {digests:?}"
        );
    }
}
