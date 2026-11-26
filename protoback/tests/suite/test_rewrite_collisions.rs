// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Two descriptors written onto one output path (specs 0437, 0447 S3).
//!
//! Under --target protoc, the descriptor in use, `internal/proto2/` of
//! package `proto2`, is written as google/protobuf/descriptor.proto. With
//! protobuf's own in the input too, the one in use is kept, the other
//! dropped with W27. No other path is rewritten, so no other collision is
//! left (spec 0437's error case went with variants).
//!
//! Ported from `test_rewrite_collisions.py`. Its inputs, built by its
//! `_inputs` with Python's protobuf (protoc's descriptor.proto as itself and
//! moved to package `proto2`, `FileOptions` gaining `internal_flag = 99` and
//! losing field 20; a user file with options added), are the fixtures
//! `rewrite_collisions/`, made once by that code
//! (`protoback/tests/fixtures/gen/test_rewrite_collisions.py`): the three sets
//! `#@prototext`, of the original's bytes (checked here), and `user.textpb`
//! as protoc printed it. protobuf's copy is the original's too: Python wrote
//! it by field number, protoc not quite, so compiling it here would not give
//! the same bytes.

use crate::common::{encode_fixture, fixture, protoback, tmp, words, Run};
use std::path::{Path, PathBuf};

const ROOT: &str = "internal/proto2/descriptor.proto";
const STOCK: &str = "google/protobuf/descriptor.proto";
const ROOT_SHA256: &str = "41de4cd68a5dc9e6fe095682b5d18111799ec07764538c6705ff9863b1390a24";
const STOCK_SHA256: &str = "acb9a01ee44e4032952d5d683ab48ae8625137cb3eca6e958d112fec693dca1b";
/// user.proto with `internal_flag = 5` (field 99, varint) among its options.
const USER_SHA256: &str = "5922751370c90133f07e0cc8a525e0b57941c3b093af289cec71a2b4978d6647";

/// The inputs in `<dir>/in`: both descriptors, and user.pb or, as text,
/// user.textpb (with `java_generate_equals_and_hash = true`, field 20).
fn inputs(dir: &Path, as_text: bool) -> PathBuf {
    let inp = dir.join("in");
    encode_fixture(
        "rewrite_collisions/internal/proto2/descriptor.pb",
        ROOT_SHA256,
        &inp.join("internal/proto2/descriptor.pb"),
    );
    encode_fixture(
        "rewrite_collisions/google/protobuf/descriptor.pb",
        STOCK_SHA256,
        &inp.join("google/protobuf/descriptor.pb"),
    );
    if as_text {
        std::fs::copy(
            fixture("rewrite_collisions/user.textpb"),
            inp.join("user.textpb"),
        )
        .unwrap();
    } else {
        encode_fixture(
            "rewrite_collisions/user.pb",
            USER_SHA256,
            &inp.join("user.pb"),
        );
    }
    inp
}

fn run(dir: &Path, inp: &Path, user: &str) -> Run {
    protoback([
        format!("-I{}", inp.display()),
        "-O".into(),
        dir.join("out").display().to_string(),
        "--target".into(),
        "protoc".into(),
        "--descriptor".into(),
        ROOT.into(),
        user.into(),
        ROOT.replace(".proto", ".pb"),
        STOCK.replace(".proto", ".pb"),
    ])
}

#[test]
fn test_the_descriptor_in_use_is_kept_and_the_other_dropped() {
    let dir = tmp("test_the_descriptor_in_use_is_kept_and_the_other_dropped");
    let inp = inputs(&dir, false);
    let r = run(&dir, &inp, "user.pb");
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let said = format!(
        "Warning [W27]: '{STOCK}' dropped: '{ROOT}', the descriptor in use, is written as '{STOCK}' (--target protoc)"
    );
    assert!(words(&r.stderr).contains(&said), "{}", r.stderr);
    let written = std::fs::read_to_string(dir.join("out").join(STOCK)).unwrap();
    assert!(written.contains("internal_flag = 99"));
    let user = std::fs::read_to_string(dir.join("out/user.proto")).unwrap();
    assert!(user.contains("option internal_flag = 5;"), "{user}");
}

/// Field 20 (java_generate_equals_and_hash) = true: protobuf's copy
/// declares it, the one in use does not. Read as text, its name is declared
/// nowhere in the schema: W4.
#[test]
fn test_a_value_only_the_dropped_copy_declares_is_said() {
    let dir = tmp("test_a_value_only_the_dropped_copy_declares_is_said");
    let inp = inputs(&dir, true);
    let r = run(&dir, &inp, "user.textpb");
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(
        words(&r.stderr)
            .contains("field java_generate_equals_and_hash of proto2.FileOptions is declared nowhere in the input: skipped"),
        "{}",
        r.stderr
    );
}
