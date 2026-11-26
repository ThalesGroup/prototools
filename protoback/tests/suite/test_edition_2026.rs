// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! An edition 2026 file renders with `edition = "2026";` (specs 0457,
//! 0460): from text, parsed against the built-in descriptor.proto of the
//! release every copy comes from.
//!
//! Ported from `test_edition_2026.py`. Its `needs_release_protobuf` marker
//! skipped it where the Python protobuf beside reproto was older than its
//! built-in descriptor.proto; protoback embeds upb at the release, so the
//! condition never holds, and the port has no marker.

use crate::common::{protoback, tmp, write};

const INPUT: &str = r#"name: "a/b.proto" package: "a" syntax: "editions" edition: EDITION_2026
message_type { name: "M" field { name: "x" number: 1 type: TYPE_INT32 json_name: "x" } }
"#;

#[test]
fn test_an_edition_2026_file_renders() {
    let dir = tmp("test_an_edition_2026_file_renders");
    write(&dir.join("in/b.textpb"), INPUT);
    let run = protoback([
        "-I".as_ref(),
        dir.join("in").as_os_str(),
        "-O".as_ref(),
        dir.join("out").as_os_str(),
    ]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        !run.stderr.contains("[W15]") && !run.stderr.contains("[W6]"),
        "{}",
        run.stderr
    );
    let text = std::fs::read_to_string(dir.join("out/a/b.proto")).unwrap();
    assert!(text.contains(r#"edition = "2026";"#), "{text}");
    assert!(text.contains("int32 x = 1;"), "{text}");
}
