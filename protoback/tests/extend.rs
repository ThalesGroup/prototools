// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0459: an `extend` block whose every field is an orphan is an orphan
//! whole, nested or top-level; one holding a field still written is not.

use std::path::Path;
use std::process::Command;

/// `x.JobSpec` is in no file: both blocks extending it hold orphans only.
/// `e.Base` is: its block holds `ok`, written, and `bad`, whose type is in
/// no file.
const INPUT: &str = r#"name: "e.proto" package: "e" dependency: "gone.proto"
message_type { name: "Base" extension_range { start: 100 end: 200 } }
message_type { name: "Ext"
  extension { name: "field" number: 109825718 label: LABEL_OPTIONAL type: TYPE_MESSAGE
    type_name: ".e.Ext" extendee: ".x.JobSpec" } }
extension { name: "top" number: 5 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".x.JobSpec" }
extension { name: "ok" number: 100 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".e.Base" }
extension { name: "bad" number: 101 label: LABEL_OPTIONAL type: TYPE_MESSAGE
  type_name: ".x.Gone" extendee: ".e.Base" }"#;

#[test]
fn an_extend_block_of_orphans_is_an_orphan() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("extend-orphans");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/e.textpb"), INPUT).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_protoback"))
        .arg("-I")
        .arg(dir.join("in"))
        .arg("-O")
        .arg(dir.join("out"))
        .env_remove("REPROTO_VARIANT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    let text = std::fs::read_to_string(dir.join("out/e.proto")).unwrap();
    for expected in [
        "message Ext {\n///extend x.JobSpec {\n/// optional Ext field = 109825718;\n///}\n}",
        "///extend x.JobSpec {\n///optional int32 top = 5;\n///}",
        "extend Base {\n  optional int32 ok = 100;\n",
    ] {
        assert!(text.contains(expected), "missing:\n{expected}\nin:\n{text}");
    }
}
