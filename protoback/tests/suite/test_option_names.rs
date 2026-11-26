// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Option names as googleapis writes them, custom features as paths (spec
//! 0439): the text, and every options message byte-identical after
//! recompiling.
//!
//! Ported from `test_option_names.py`. Each element's options are the bytes
//! protoc wrote for them, read by a wire walk, where the original parsed the
//! set with Python's protobuf and serialized each options message again:
//! both sides are protoc's, and protobuf writes known fields by number and
//! keeps unknown ones in their order, so the comparison is the same.

use crate::common::{compile_set, delimited, name_and_options, protoback, tmp, tools, write};
use std::collections::BTreeMap;
use std::path::Path;

const FILES: &[(&str, &str)] = &[
    (
        "ext.proto",
        r#"syntax = "proto2";
package ext;
import "google/protobuf/descriptor.proto";
message Rule { optional string get = 1; optional string body = 2; }
extend google.protobuf.FileOptions { optional int32 f_opt = 50001; }
extend google.protobuf.MessageOptions {
  optional int32 m_opt = 50002;
  optional Rule rule = 50006;
}
extend google.protobuf.FieldOptions { optional string fld = 50003; }
extend google.protobuf.EnumValueOptions { optional bool ev = 50004; }
extend google.protobuf.MethodOptions { optional int32 meth = 50005; }
"#,
    ),
    (
        "u.proto",
        r#"syntax = "proto2";
package u;
import "ext.proto";
option (ext.f_opt) = 1;
message M {
  option (ext.m_opt) = 2;
  option (ext.rule) = { get: "/v1/x" body: "*" };
  optional string s = 1 [(ext.fld) = "s"];
}
enum E { E_ZERO = 0 [(ext.ev) = true]; }
service S { rpc Get(M) returns (M) { option (ext.meth) = 3; } }
"#,
    ),
    // In package y.ext, `ext.f_opt` is captured: its first component finds
    // the package y.ext first, which has no f_opt.
    (
        "y/c.proto",
        r#"syntax = "proto2";
package y.ext;
import "ext.proto";
option (.ext.f_opt) = 4;
message C { optional int32 x = 1; }
"#,
    ),
    (
        "f/feat.proto",
        r#"edition = "2023";
package feat;
import "google/protobuf/cpp_features.proto";
option features.(pb.cpp).string_type = CORD;
message M {
  string s = 1 [features.(pb.cpp).legacy_closed_enum = true, features.(pb.cpp).string_type = VIEW];
  string t = 2 [features.(pb.cpp) = {}];
}
"#,
    ),
];

fn compile(root: &Path, out: &Path) -> Vec<u8> {
    let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
    compile_set(root, &names, out)
}

/// Every element's options, by where it is (`_options`).
fn options(set: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for f in delimited(set, 1) {
        let (file, opts) = name_and_options(f, 8);
        if !FILES.iter().any(|(n, _)| *n == file) {
            continue;
        }
        out.insert(file.clone(), opts);
        for m in delimited(f, 4) {
            let (mname, opts) = name_and_options(m, 7);
            out.insert(format!("{file}:{mname}"), opts);
            for fd in delimited(m, 2) {
                let (fname, opts) = name_and_options(fd, 8);
                out.insert(format!("{file}:{mname}.{fname}"), opts);
            }
        }
        for e in delimited(f, 5) {
            for v in delimited(e, 2) {
                let (vname, opts) = name_and_options(v, 3);
                out.insert(format!("{file}:{vname}"), opts);
            }
        }
        for s in delimited(f, 6) {
            let (sname, _) = name_and_options(s, 3);
            for meth in delimited(s, 2) {
                let (mname, opts) = name_and_options(meth, 4);
                out.insert(format!("{file}:{sname}.{mname}"), opts);
            }
        }
    }
    out
}

#[test]
fn test_option_names_and_feature_paths() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_option_names_and_feature_paths");
    let src = dir.join("src");
    for (name, text) in FILES {
        write(&src.join(name), text);
    }
    let original = compile(&src, &dir.join("original.pb"));
    let out = dir.join("out");
    let r = protoback([
        "-q".into(),
        format!("-I{}", dir.join("original.pb").display()),
        "-O".into(),
        out.display().to_string(),
        "--use-variant".into(),
        "descriptor".into(),
        ".".into(),
    ]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let again = compile(&out, &dir.join("again.pb"));
    assert_eq!(options(&again), options(&original));

    let rendered = |name: &str| {
        std::fs::read_to_string(out.join(name))
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let u = rendered("u.proto");
    // In full, without the leading dot (googleapis' way).
    for written in [
        "option (ext.f_opt) = 1;",
        "option (ext.m_opt) = 2;",
        r#"(ext.fld) = "s""#,
        "(ext.ev) = true",
        "option (ext.meth) = 3;",
        "option (ext.rule) = {",
    ] {
        assert!(u.contains(written), "{written}");
    }
    assert!(!u.contains("(.ext."));
    // Captured: the dot kept.
    assert!(rendered("y/c.proto").contains("option (.ext.f_opt) = 4;"));
    // A custom feature: one path per field (protobuf's way); empty, {}.
    let feat = rendered("f/feat.proto");
    assert!(feat.contains("option features.(pb.cpp).string_type = CORD;"));
    assert!(feat.contains(
        "features.(pb.cpp).legacy_closed_enum = true, features.(pb.cpp).string_type = VIEW"
    ));
    assert!(feat.contains("features.(pb.cpp) = {}"));
}
