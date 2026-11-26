// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Tests for schema-db transitive completeness (spec 0150).
//!
//! Regression test for a bug where --schema-db-out/--build-schema-db
//! silently omitted google/protobuf/descriptor.proto (or a variant's
//! equivalent) from the produced FileDescriptorSet whenever it was needed
//! only as a custom-option dependency and --no-emit-descriptor was passed,
//! violating the transitive-completeness invariant required by consumers
//! such as prost_reflect::DescriptorPool::decode.
//!
//! Ported from `test_schema_db.py`; its test's name lower-cased.

use crate::common::{fixture, protoback, protoc, set_files, tmp, tools};
use std::path::{Path, PathBuf};

fn compile(orig: &Path, proto: &str) -> PathBuf {
    let pb = orig.join(Path::new(proto).with_extension("pb").file_name().unwrap());
    protoc([
        format!("--descriptor_set_out={}", pb.display()),
        format!("-I{}", fixture("").display()),
        fixture(proto).display().to_string(),
    ]);
    pb
}

/// --schema-db-out with --no-emit-descriptor must still include
/// google/protobuf/descriptor.proto when a custom option pulls it in (spec
/// 0150), while --proto-out continues to omit it (N1).
#[test]
fn test_g1_schema_db_includes_suppressed_descriptor_proto() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_g1_schema_db_includes_suppressed_descriptor_proto");
    let (orig, out, db) = (dir.join("orig"), dir.join("out"), dir.join("schema.desc"));
    std::fs::create_dir_all(&orig).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let main = compile(&orig, "editions_roundtrip.proto");
    let dep = compile(&orig, "editions_custom_option_dep.proto");
    let weak = compile(&orig, "weak_import_proto2_dep.proto");
    let r = protoback([
        "--use-variant".into(),
        "descriptor".into(),
        "--no-emit-descriptor".into(),
        format!("-I{}", fixture("").display()),
        format!("--proto-out={}", out.display()),
        format!("--schema-db-out={}", db.display()),
        main.display().to_string(),
        dep.display().to_string(),
        weak.display().to_string(),
    ]);
    assert_eq!(r.code, Some(0), "protoback failed:\n{}", r.stderr);
    assert!(db.exists(), "schema DB not created");

    let files = set_files(&db);
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"google/protobuf/descriptor.proto"),
        "descriptor.proto missing from schema DB; files present: {names:?}"
    );
    // Transitive completeness: every dependency listed by an included file
    // must itself be included (mirrors prost_reflect's own requirement, the
    // exact invariant this bug violated).
    for f in &files {
        for dep in &f.dependencies {
            assert!(
                names.contains(&dep.as_str()),
                "{} depends on {dep}, which is missing from the schema DB",
                f.name
            );
        }
    }
    // N1: --proto-out still does not contain descriptor.proto as text.
    assert!(!out.join("google/protobuf/descriptor.proto").exists());
}
