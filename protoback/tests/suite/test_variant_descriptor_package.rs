// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The options types come from the descriptor's own descriptor.proto.
//!
//! --descriptor names the descriptor.proto by path (spec 0447 S1); the
//! package that file declares is the package of the options types protoback
//! parses options with, and of the nine `*Options` a proto3 file may extend
//! (A5). Neither is assumed from the path: here the descriptor.proto is
//! `alt/descriptor.proto`, `package altpb`.
//!
//! Ported from `test_variant_descriptor_package.py`. Its input, built by its
//! `_alt_descriptor` and `_set` with Python's protobuf (that release's own
//! descriptor.proto, renamed and repackaged; a proto2 user file; a proto3
//! file extending `altpb.FieldOptions`), is the `#@prototext` fixture
//! `variant_descriptor_package/set.pb`, made once by that code with protobuf
//! 7.34.1 (`protoback/tests/fixtures/gen/test_variant_descriptor_package.py`).

use crate::common::{encode_fixture, protoback, tmp, words, Run};
use std::path::{Path, PathBuf};

const NAME: &str = "alt/descriptor.proto";
const SET_SHA256: &str = "ea44683803717f2b0f6d88c0c6a263e41f3a22b571906a0bcf4f6537617ec30e";

fn run(dir: &Path) -> (Run, PathBuf) {
    let pb = dir.join("in/set.pb");
    encode_fixture("variant_descriptor_package/set.pb", SET_SHA256, &pb);
    let out = dir.join("out");
    let r = protoback([
        "--descriptor".into(),
        NAME.into(),
        format!("-I{}", pb.parent().unwrap().display()),
        format!("--proto-out={}", out.display()),
        "set.pb".into(),
    ]);
    (r, out)
}

#[test]
fn test_options_parse_with_the_descriptors_own_package() {
    let (r, out) = run(&tmp("test_options_parse_with_the_descriptors_own_package"));
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(out.join("use.proto")).unwrap();
    assert!(text.contains("[deprecated = true]"), "{text}");
}

/// A5 allows extending the nine *Options of the input's own
/// descriptor.proto, whatever its package.
#[test]
fn test_a_proto3_file_may_extend_the_descriptors_options() {
    let (r, out) = run(&tmp(
        "test_a_proto3_file_may_extend_the_descriptors_options",
    ));
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let text = std::fs::read_to_string(out.join("ext3.proto")).unwrap();
    assert!(!text.contains("not valid in proto3"), "{text}");
    assert!(
        !words(&r.stderr).contains("not valid in proto3"),
        "{}",
        r.stderr
    );
    assert!(text.contains("FieldOptions {"), "{text}");
    assert!(text.contains("int32 sensitivity = 50001;"), "{text}");
}
