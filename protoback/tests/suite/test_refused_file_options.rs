// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A file the descriptor pool refuses keeps its file options, its features
//! among them (spec 0475): rendered from its descriptor, W19 still
//! reported; a custom option whose extension the refused file itself
//! declares, an orphan with W22.
//!
//! Ported from `test_refused_file_options.py`.

use crate::common::{protoback, tmp, write};

const ROW: &str = r#"name: "a/row.proto" package: "a" syntax: "editions" edition: EDITION_2023
message_type { name: "Base" extension_range { start: 100 end: 200 } }
"#;

/// Refused: extension 500 is in no extension range of a.Base.
const USER: &str = r#"name: "b/user.proto" package: "b" dependency: "a/row.proto"
dependency: "google/protobuf/descriptor.proto"
syntax: "editions" edition: EDITION_2023
options { java_package: "com.example.b" features { enum_type: CLOSED } 50001: 5 }
enum_type { name: "E" value { name: "E_ONE" number: 1 } }
extension { name: "far" number: 500 type: TYPE_INT32 extendee: ".a.Base" json_name: "far" }
extension { name: "tag" number: 50001 type: TYPE_INT32
  extendee: ".google.protobuf.FileOptions" json_name: "tag" }
"#;

#[test]
fn test_a_refused_file_keeps_its_options() {
    let dir = tmp("test_a_refused_file_keeps_its_options");
    write(&dir.join("in/row.textpb"), ROW);
    write(&dir.join("in/user.textpb"), USER);
    let run = protoback([
        "-I".as_ref(),
        dir.join("in").as_os_str(),
        "-O".as_ref(),
        dir.join("out").as_os_str(),
    ]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains("[W19]") && run.stderr.contains("[W22]"),
        "{}",
        run.stderr
    );
    let text = std::fs::read_to_string(dir.join("out/b/user.proto")).unwrap();
    for expected in [
        "// WARNING[render]: the descriptor pool refused it: field number 500",
        "option features.enum_type = CLOSED;",
        r#"option java_package = "com.example.b";"#,
        "///option <50001> = 5;",
    ] {
        assert!(text.contains(expected), "missing: {expected}\n{text}");
    }
    assert!(!text.contains("OMITTED[render]"), "{text}");
}
