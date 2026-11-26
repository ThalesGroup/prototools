// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A proto3 `optional` field written as proto2 (spec 0429 S13).
//!
//! Under --force-proto2-output, proto3's `optional` is proto2's: a field with
//! presence, no synthetic oneof. The text says `optional int32 x = 1;`, and
//! the descriptor written with -b is the one protoc compiles from that text:
//! no `proto3_optional` (which protoc refuses in proto2), no synthetic oneof,
//! the real oneofs kept.
//!
//! Ported from `test_proto3_optional_as_proto2.py`, whose comparison of the
//! two messages (Python's `==`) is their deterministic encodings' equality.

use crate::common::{message_type, protoback, protoc, tmp, tools, write};

const SOURCE: &str = r#"syntax = "proto3";
package p3;
message M {
  optional int32 x = 1;
  int32 y = 2;
  oneof choice {
    string a = 3;
    int64 b = 4;
  }
  optional string z = 5;
}
"#;

#[test]
fn test_proto3_optional_is_proto2_optional() {
    if !tools(&["protoc"]) {
        return;
    }
    let dir = tmp("test_proto3_optional_is_proto2_optional");
    let src = dir.join("src");
    write(&src.join("m.proto"), SOURCE);
    protoc([
        format!("-I{}", src.display()),
        format!("--descriptor_set_out={}", dir.join("m.pb").display()),
        "m.proto".into(),
    ]);
    let out = dir.join("out");
    let run = protoback([
        "-q".as_ref(),
        "-b".as_ref(),
        "--no-source-info".as_ref(),
        "--force-proto2-output".as_ref(),
        "--use-variant".as_ref(),
        "descriptor".as_ref(),
        format!("-I{}", dir.display()).as_ref(),
        "-O".as_ref(),
        out.as_os_str(),
        "m.pb".as_ref(),
    ]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);

    let text = std::fs::read_to_string(out.join("m.proto")).unwrap();
    assert!(text.contains("optional int32 x = 1;"));
    assert!(text.contains("optional string z = 5;"));
    assert!(!text.contains("oneof _"));
    assert!(text.contains("oneof choice {"));

    // The emitted descriptor is the one protoc compiles from the text.
    let again = dir.join("again.pb");
    protoc([
        format!("-I{}", out.display()),
        format!("--descriptor_set_out={}", again.display()),
        "m.proto".into(),
    ]);
    let set = std::fs::read(&again).unwrap();
    let emitted = std::fs::read(out.join("m.pb")).unwrap();
    assert_eq!(
        message_type(&emitted, false, 0),
        message_type(&set, true, 0),
        "the emitted descriptor's first message is not protoc's"
    );
}
